#include "live.hpp"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <csignal>
#include <condition_variable>
#include <limits>
#include <stdexcept>
#include <thread>
#include <unistd.h>
#include <sys/stat.h>

namespace rms {
static_assert(std::atomic<double>::is_always_lock_free, "callback telemetry must be lock-free");
static_assert(std::atomic<uint64_t>::is_always_lock_free, "callback counters must be lock-free");
static_assert(std::atomic<bool>::is_always_lock_free, "callback gate must be lock-free");
namespace {
volatile std::sig_atomic_t interrupted = 0;
void interrupt(int) { interrupted = 1; }
void quiet_fluid(int, const char*, void*) {}
class EditorKeepalive {
    Wire& wire_;
    const Envelope identity_;
    std::atomic<bool> stopping_{false}, failed_{false};
    std::mutex mutex_;
    std::condition_variable wakeup_;
    std::thread worker_;
public:
    EditorKeepalive(Wire& wire, const Envelope& identity, bool enabled)
        : wire_(wire), identity_(identity) {
        if (enabled) worker_ = std::thread([this] {
            try {
                for (;;) {
                    std::unique_lock<std::mutex> lock(mutex_);
                    if (wakeup_.wait_for(lock,std::chrono::milliseconds(250),[this] { return stopping_.load(); }))
                        return;
                    lock.unlock();
                    if (plugin_editor_callback_active()) {
                        auto message = identity_.message("editor_keepalive");
                        message["editor_busy"] = true;
                        wire_.send(message); // Serialized with normal main-thread frames.
                    }
                }
            } catch (const std::exception&) {
                failed_.store(true);
            }
        });
    }
    ~EditorKeepalive() {
        stopping_.store(true);
        wakeup_.notify_all();
        if (worker_.joinable()) worker_.join();
    }
    bool failed() const noexcept { return failed_.load(); }
    EditorKeepalive(const EditorKeepalive&) = delete;
    EditorKeepalive& operator=(const EditorKeepalive&) = delete;
};
}
Json Live::operation(const std::string& type, const Json& j) {
    // Control operations always own a quiescent graph; errors release the gate.
    quiesce();
    control_hold_.store(true);
    struct Resume { std::atomic<bool>& gate; ~Resume() { gate.store(false); } } resume{quiescent_};
    if (type == "checkpoint") {
        if (engine_->state() == State::playing) throw std::runtime_error("checkpoint requires paused/stopped playback");
        auto directory = text(j, "directory", 4096);
        absolute(directory);
        struct stat st{};
        if (lstat(directory.c_str(), &st) || !S_ISDIR(st.st_mode) || st.st_uid != getuid() || (st.st_mode & 0777) != 0700)
            throw std::runtime_error("checkpoint directory must be owned private 0700 directory without final symlink");
        Json parts = Json::array();
        uint64_t total = 0;
        auto descriptor = [](const StateFile& f) -> Json {
            return {{"path",f.path},{"sha256",f.sha256},{"bytes",f.bytes}};
        };
        for (size_t i = 0; i < nodes_.size(); ++i) if (auto* plugin = dynamic_cast<Vst3*>(nodes_[i].get())) {
            auto state = plugin->capture();
            total += state.first.size() + state.second.size();
            if (total > 256 * 1024 * 1024) throw std::runtime_error("checkpoint exceeds 256MiB");
            std::string prefix = directory + "/part-" + std::to_string(i);
            Json a = nullptr, b = nullptr;
            if (!state.first.empty()) a = descriptor(write_private(prefix + "-component.bin", state.first));
            if (!state.second.empty()) b = descriptor(write_private(prefix + "-controller.bin", state.second));
            parts.push_back({{"part_id",plan_.parts[i].id},{"class_id",plan_.parts[i].source.class_id},
                {"component_state",a},{"controller_state",b}});
        }
        for (auto& n : nodes_) if (auto* plugin = dynamic_cast<Vst3*>(n.get())) plugin->clean();
        return {{"type","checkpoint_result"},{"parts",parts}};
    }
    auto id = text(j,"part_id",256);
    auto found = std::find_if(plan_.parts.begin(),plan_.parts.end(),[&](const Part& p) { return p.id == id; });
    if (found == plan_.parts.end()) throw std::runtime_error("unknown part_id");
    size_t index = static_cast<size_t>(found - plan_.parts.begin());
    auto* plugin = dynamic_cast<Vst3*>(nodes_[index].get());
    if (!plugin) throw std::runtime_error("operation requires VST3 instrument source");
    if (type == "gain") engine_->gain(index,number(j,"gain",0,1));
    else if (type == "parameters") {
        auto offset = integer(j,"offset",UINT32_MAX), limit = integer(j,"limit",128);
        if (!limit) throw std::runtime_error("parameter limit must be 1..128");
        auto result = plugin->parameters(static_cast<uint32_t>(offset),static_cast<uint32_t>(limit));
        result["type"] = "parameters_result"; result["part_id"] = id; return result;
    } else if (type == "parameter") plugin->parameter(static_cast<uint32_t>(integer(j,"param_id",UINT32_MAX)),number(j,"value",0,1));
    else if (type == "preset_load") {
        if (engine_->state() == State::playing) throw std::runtime_error("preset requires paused/stopped playback");
        plugin->preset(text(j,"path",4096));
    } else if (type == "editor_open" || type == "editor_close") plugin->editor(type == "editor_open","RustyMusicSheets — "+id);
    else throw std::runtime_error("unsupported plugin operation");
    return {{"type","ack"},{"operation",type},{"part_id",id}};
}
SoundFont::SoundFont(const std::string& path, uint32_t rate) {
    settings_ = new_fluid_settings();
    if (!settings_) throw std::runtime_error("cannot allocate FluidSynth settings");
    // No FluidSynth audio driver, background synthesis, synth mutex or lazy disk I/O.
    fluid_settings_setnum(settings_, "synth.sample-rate", rate);
    fluid_settings_setnum(settings_, "synth.gain", 0.2);
    fluid_settings_setint(settings_, "synth.threadsafe-api", 0);
    fluid_settings_setint(settings_, "synth.cpu-cores", 1);
    fluid_settings_setint(settings_, "synth.polyphony", 256);
    fluid_settings_setint(settings_, "synth.dynamic-sample-loading", 0);
    // M0 disables delay effects so pause/seek can flush all audio immediately.
    fluid_settings_setint(settings_, "synth.reverb.active", 0);
    fluid_settings_setint(settings_, "synth.chorus.active", 0);
    synth_ = new_fluid_synth(settings_);
    if (!synth_ || fluid_synth_sfload(synth_, path.c_str(), 1) < 0) {
        if (synth_) delete_fluid_synth(synth_);
        delete_fluid_settings(settings_);
        synth_ = nullptr;
        settings_ = nullptr;
        throw std::runtime_error("cannot load SoundFont (check path, format and available memory)");
    }
    // Force the initial synthesis allocation/tables outside the callback.
    float l[64]{}, r[64]{};
    fluid_synth_write_float(synth_, 64, l, 0, 1, r, 0, 1);
}
SoundFont::~SoundFont() {
    if (synth_) delete_fluid_synth(synth_);
    if (settings_) delete_fluid_settings(settings_);
}
void SoundFont::note_on(uint8_t pitch, uint8_t velocity) noexcept {
    if (fluid_synth_noteon(synth_, 0, pitch, velocity) != FLUID_OK) failed_.store(true);
}
void SoundFont::note_off(uint8_t pitch) noexcept { fluid_synth_noteoff(synth_, 0, pitch); }
void SoundFont::silence() noexcept {
    fluid_synth_cc(synth_, 0, 64, 0);
    fluid_synth_all_sounds_off(synth_, 0);
    // Drain at most one internal synthesis quantum of cached pre-silence samples.
    // No host allocation; effects are disabled above.
    float l[64]{}, r[64]{};
    fluid_synth_write_float(synth_, 64, l, 0, 1, r, 0, 1);
}
void SoundFont::configure(uint8_t volume, uint8_t program) noexcept {
    fluid_synth_cc(synth_, 0, 7, volume); // legacy CC7; no duplicate post-part gain
    if (program_ != program) {
        if (fluid_synth_program_change(synth_, 0, program) != FLUID_OK) failed_.store(true);
        program_ = program;
    }
}
void SoundFont::render(float* left, float* right, uint32_t frames) noexcept {
    // Public FluidSynth has an internal (normally 64-frame) synthesis quantum.
    // Scheduler dispatch is sample-exact; acoustic event onset needs live validation
    // and must not be represented as stronger than FluidSynth's own resolution.
    if (fluid_synth_write_float(synth_, static_cast<int>(frames), left, 0, 1, right, 0, 1) != FLUID_OK) {
        std::fill_n(left, frames, 0);
        std::fill_n(right, frames, 0);
        failed_.store(true);
    }
}
Live::Live(Plan p, const std::string& soundfont, double master, double scale, bool autoplay)
    : plan_(std::move(p)) {
    for (int level = FLUID_PANIC; level <= FLUID_DBG; ++level)
        fluid_set_log_function(level, quiet_fluid, nullptr); // never log from synth callback
    try {
        jack_status_t status{};
        client_ = jack_client_open("rms-audio-host", JackNoStartServer, &status);
        if (!client_) throw std::runtime_error("JACK unavailable; start JACK or launch helper with /usr/bin/pw-jack");
        rate_ = jack_get_sample_rate(client_);
        if (rate_ < 8000 || rate_ > 384000 || jack_get_buffer_size(client_) > capacity)
            throw std::runtime_error("unsupported JACK sample rate or buffer size");
        left_ = jack_port_register(client_, "out_l", JACK_DEFAULT_AUDIO_TYPE, JackPortIsOutput, 0);
        right_ = jack_port_register(client_, "out_r", JACK_DEFAULT_AUDIO_TYPE, JackPortIsOutput, 0);
        if (!left_ || !right_) throw std::runtime_error("cannot register JACK stereo output ports");
        const char** ports = jack_get_ports(client_, nullptr, JACK_DEFAULT_AUDIO_TYPE,
                                           JackPortIsPhysical | JackPortIsTerminal | JackPortIsInput);
        if (!ports || !ports[0] || !ports[1]) {
            if (ports) jack_free(ports);
            throw std::runtime_error("no stereo physical terminal JACK playback ports; select/enable an output device");
        }
        std::string destination_l = ports[0], destination_r = ports[1];
        jack_free(ports);
        std::vector<Instrument*> instruments;
        uint64_t state_bytes = 0;
        for (size_t i = 0; i < plan_.parts.size(); ++i) {
            const auto& s = plan_.parts[i].source;
            state_bytes += s.component.bytes + s.controller.bytes;
            if (state_bytes > 256 * 1024 * 1024) throw std::runtime_error("session state exceeds 256MiB");
            if (s.vst3) nodes_.push_back(std::make_unique<Vst3>(s, rate_, capacity));
            else nodes_.push_back(std::make_unique<SoundFont>(soundfont, rate_));
            instruments.push_back(nodes_.back().get());
        }
        engine_ = std::make_unique<Engine>(plan_, rate_, capacity, instruments);
        engine_->tempo(scale);
        engine_->master(master);
        if (autoplay) engine_->play();
        publish();
        if (jack_set_process_callback(client_, process, this) ||
            jack_set_sample_rate_callback(client_, rate_changed, this) ||
            jack_set_xrun_callback(client_, xrun, this))
            throw std::runtime_error("cannot register JACK callbacks");
        jack_on_shutdown(client_, shutdown, this);
        if (jack_activate(client_)) throw std::runtime_error("cannot activate JACK client");
        int a = jack_connect(client_, jack_port_name(left_), destination_l.c_str());
        int b = jack_connect(client_, jack_port_name(right_), destination_r.c_str());
        if (a || b) throw std::runtime_error("cannot connect JACK playback ports; check device routing");
        jack_latency_range_t l{}, r{};
        jack_port_get_latency_range(left_, JackPlaybackLatency, &l);
        jack_port_get_latency_range(right_, JackPlaybackLatency, &r);
        latency_.store(std::max(l.max, r.max) + engine_->plugin_latency());
        if (latency_.load() > uint64_t(rate_) * 10)
            throw std::runtime_error("JACK downstream playback latency exceeds supported ten-second profile");
        control_hold_.store(true);
        quiescent_.store(false);
    } catch (...) { cleanup(); throw; }
}
void Live::cleanup() noexcept {
    quiescent_.store(true);
    if (client_) {
        // One deactivation at teardown, never per transport/mix command.
        jack_deactivate(client_);
        jack_client_close(client_);
        client_ = nullptr;
    }
    engine_.reset();
    nodes_.clear();
}
Live::~Live() { cleanup(); }
void Live::shutdown(void* data) noexcept { static_cast<Live*>(data)->lost_.store(true); }
int Live::rate_changed(jack_nframes_t rate, void* data) noexcept {
    auto* self = static_cast<Live*>(data);
    if (rate != self->rate_) self->lost_.store(true);
    return 0;
}
int Live::xrun(void* data) noexcept {
    static_cast<Live*>(data)->xruns_.fetch_add(1);
    return 0;
}
void Live::publish() noexcept {
    telemetry_sequence_.fetch_add(1);
    position_.store(engine_->audible(), std::memory_order_relaxed);
    rendered_.store(engine_->position(), std::memory_order_relaxed);
    duration_.store(engine_->duration(), std::memory_order_relaxed);
    scale_.store(engine_->scale(), std::memory_order_relaxed);
    tick_.store(engine_->score_tick(), std::memory_order_relaxed);
    state_.store(engine_->state(), std::memory_order_relaxed);
    telemetry_sequence_.fetch_add(1);
}
int Live::process(jack_nframes_t frames, void* data) noexcept {
    auto* self = static_cast<Live*>(data);
    auto* left = static_cast<float*>(jack_port_get_buffer(self->left_, frames));
    auto* right = static_cast<float*>(jack_port_get_buffer(self->right_, frames));
    std::fill_n(left, frames, 0);
    std::fill_n(right, frames, 0);
    self->in_callback_.store(true);
    // This exact device axis belongs only to the callback and is never reset by
    // control edits. Quiescent callbacks may count silence but cannot touch the
    // graph while the control thread owns it.
    jack_nframes_t now = jack_last_frame_time(self->client_);
    jack_nframes_t gap = self->clock_started_ ? now - self->expected_frame_ : 0;
    self->clock_started_ = true;
    self->expected_frame_ = now + frames;
    if (gap >= (uint32_t{1} << 31)) self->lost_.store(true);
    if (self->quiescent_.load()) {
        self->pending_hold_frames_.fetch_add(uint64_t(gap) + frames);
    } else if (!self->lost_.load()) {
        if (frames > capacity) self->lost_.store(true);
        else {
            uint64_t hold = self->pending_hold_frames_.exchange(0);
            bool controlled = self->control_hold_.exchange(false);
            // A skipped deadline overlapping a deliberate graph edit is silence,
            // not musical catch-up. Ordinary xruns advance the musical clock.
            if (controlled) hold += gap;
            self->engine_->hold(hold);
            if (!controlled) self->engine_->advance_gap(gap);
            self->engine_->process(left, right, frames, self->latency_.load(std::memory_order_relaxed));
            if (!self->engine_->mapping_valid()) {
                self->mapping_failed_.store(true);
                self->lost_.store(true);
            }
            for (auto& node : self->nodes_) if (node->failed()) self->lost_.store(true);
            self->publish();
        }
    }
    if (self->lost_.load()) {
        std::fill_n(left, frames, 0);
        std::fill_n(right, frames, 0);
    }
    self->in_callback_.store(false);
    return 0;
}
void Live::quiesce() {
    quiescent_.store(true);
    auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(500);
    while (in_callback_.load()) {
        if (std::chrono::steady_clock::now() >= deadline)
            throw std::runtime_error("audio callback did not quiesce; terminate helper for recovery");
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
}
void Live::apply(const std::string& type, const Json& j) {
    // Validate all fields before touching the graph.
    double value = 0;
    size_t part = 0;
    bool enabled = false;
    uint8_t volume = 0, program = 0;
    if (type == "seek") value = number(j, "delta", -std::numeric_limits<double>::max(),
                                     std::numeric_limits<double>::max());
    else if (type == "tempo") value = number(j, "scale", 0.1, 4);
    else if (type == "master_volume") value = number(j, "percent", 0, 100);
    else if (type == "mix") {
        auto id = text(j, "part_id", 256);
        auto found = std::find_if(plan_.parts.begin(), plan_.parts.end(), [&](const Part& p) { return p.id == id; });
        if (found == plan_.parts.end()) throw std::runtime_error("unknown part_id");
        part = static_cast<size_t>(found - plan_.parts.begin());
        enabled = boolean(j, "enabled");
        volume = static_cast<uint8_t>(integer(j, "volume", 127));
        program = static_cast<uint8_t>(integer(j, "program", 127));
    }
    quiesce();
    control_hold_.store(true);
    if (mapping_failed_.load())
        throw std::runtime_error("JACK latency mapping exceeded ten-second profile or retained transport-history capacity");
    if (lost_.load()) throw std::runtime_error("JACK device/rate changed or synth failed; restart audio backend");
    if (type == "play") engine_->play();
    else if (type == "pause") engine_->pause();
    else if (type == "stop") engine_->stop();
    else if (type == "seek") engine_->seek(value);
    else if (type == "tempo") engine_->tempo(value);
    else if (type == "master_volume") engine_->master(value);
    else if (type == "mix") engine_->mix(part, enabled, volume, program);
    else throw std::runtime_error("unsupported graph command");
    publish();
    quiescent_.store(false);
}
Json Live::status(const Envelope& envelope) {
    if (mapping_failed_.load())
        throw std::runtime_error("JACK latency mapping exceeded ten-second profile or retained transport-history capacity");
    if (lost_.load()) throw std::runtime_error("JACK device/rate changed or synth failed; restart audio backend");
    if (!jack_port_connected(left_) || !jack_port_connected(right_))
        throw std::runtime_error("JACK playback output disconnected; reconnect device and restart audio backend");
    jack_latency_range_t l{}, r{};
    jack_port_get_latency_range(left_, JackPlaybackLatency, &l);
    jack_port_get_latency_range(right_, JackPlaybackLatency, &r);
    latency_.store(std::max(l.max, r.max) + engine_->plugin_latency());
    if (latency_.load() > uint64_t(rate_) * 10)
        throw std::runtime_error("JACK downstream playback latency exceeds supported ten-second profile");
    // All payload members remain atomic (a seqlock over plain C++ fields would
    // still constitute a data race). Only this non-RT reader may retry.
    State state;
    double position, rendered, duration, scale;
    uint64_t tick;
    auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(20);
    while (true) {
        uint64_t before = telemetry_sequence_.load();
        position = position_.load();
        rendered = rendered_.load();
        duration = duration_.load();
        scale = scale_.load();
        tick = tick_.load();
        state = state_.load();
        if (!(before & 1) && telemetry_sequence_.load() == before) break;
        if (std::chrono::steady_clock::now() >= deadline)
            throw std::runtime_error("audio telemetry stalled; restart audio backend");
        std::this_thread::yield();
    }
    auto j = envelope.message("status");
    j["state"] = state_name(state);
    j["position"] = position;
    j["rendered_position"] = rendered;
    j["duration"] = duration;
    j["score_tick"] = tick;
    j["tempo_scale"] = scale;
    j["sample_rate"] = rate_;
    j["latency_frames"] = latency_.load();
    j["xruns"] = xruns_.load();
    j["dirty_parts"] = Json::array();
    quiesce();
    control_hold_.store(true);
    struct Resume { std::atomic<bool>& gate; ~Resume() { gate.store(false); } } resume{quiescent_};
    bool reconfigured = false;
    for (size_t i = 0; i < nodes_.size(); ++i)
        if (auto* plugin = dynamic_cast<Vst3*>(nodes_[i].get())) {
            reconfigured = plugin->pump() || reconfigured;
            if (plugin->dirty()) j["dirty_parts"].push_back(plan_.parts[i].id);
        }
    if (reconfigured) {
        try { engine_->rebuild_latency(); }
        catch (...) { lost_.store(true); throw; }
        latency_.store(std::max(l.max,r.max)+engine_->plugin_latency());
        if (latency_.load() > uint64_t(rate_)*10) {
            lost_.store(true); throw std::runtime_error("combined latency exceeds supported ten seconds");
        }
        publish();
        j["state"] = state_name(engine_->state());
        j["position"] = engine_->audible(); j["rendered_position"] = engine_->position();
        j["score_tick"] = engine_->score_tick(); j["latency_frames"] = latency_.load();
    }
    return j;
}
int serve(const std::string& path, pid_t parent, const std::string& mode) {
    Wire wire(path, parent);
    Envelope envelope;
    std::unique_ptr<Live> live;
    std::signal(SIGTERM, interrupt);
    std::signal(SIGINT, interrupt);
    try {
        Json message;
        auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
        while (!wire.receive(message, 40)) {
            if (interrupted || std::chrono::steady_clock::now() >= deadline)
                throw std::runtime_error("parent hello timeout");
        }
        envelope.hello(message);
        auto hello = envelope.message("hello");
        hello["capabilities"] = plugin_capabilities();
        wire.send(hello);
        const auto& capabilities = hello["capabilities"];
        bool editor_keepalive = mode == "serve" && envelope.editor_keepalive &&
            std::find(capabilities.begin(),capabilities.end(),Json("editor_keepalive")) != capabilities.end();
        // The immutable hello identity has request/epoch zero. These frames
        // attest only to an active UI callback and never acknowledge commands.
        EditorKeepalive keepalive(wire,envelope,editor_keepalive);
        auto next_status = std::chrono::steady_clock::now();
        while (!interrupted && getppid() == parent) {
            if (keepalive.failed()) throw std::runtime_error("editor keepalive socket failed");
            if (wire.receive(message, 10)) {
                Envelope next = envelope;
                std::string type = next.command(message);
                if (type == "shutdown") { live.reset(); return 0; }
                if (mode == "probe") throw std::runtime_error("probe accepts hello then shutdown only");
                if (mode == "scan") {
                    if (type != "scan" || next.request != 1) throw std::runtime_error("scan accepts one scan request only");
                    Json reply = next.message("scan_result");
                    try { reply.update(scan_plugin(message)); }
                    catch (const std::exception& error) {
                        reply = next.message("operation_error"); reply["operation"] = "scan";
                        reply["part_id"] = nullptr; reply["message"] = std::string(error.what()).substr(0,1024);
                    }
                    wire.send(reply); envelope = next; continue;
                }
                bool operation = type == "gain" || type == "parameters" || type == "parameter" || type == "checkpoint" ||
                    type == "preset_load" || type == "editor_open" || type == "editor_close";
                if (operation) {
                    Json reply;
                    try {
                        if (!live) throw std::runtime_error("load a playback graph first");
                        reply = live->operation(type,message);
                    } catch (const std::exception& error) {
                        reply = {{"type","operation_error"},{"operation",type},{"part_id",message.value("part_id",Json(nullptr))},
                            {"message",std::string(error.what()).substr(0,1024)}};
                    }
                    auto complete = next.message("ack"); complete.update(reply); wire.send(complete);
                    envelope = next; continue;
                }
                if (!live) {
                    if (type != "load" || next.request != 1 || next.epoch != 0)
                        throw std::runtime_error("expected load at request one, epoch zero");
                    std::string soundfont;
                    bool autoplay = boolean(message, "autoplay");
                    double master = number(message, "master_volume", 0, 100);
                    double scale = number(message, "tempo_scale", 0.1, 4);
                    Plan plan = read_plan(message);
                    if (std::any_of(plan.parts.begin(),plan.parts.end(),[](const Part& p) { return !p.source.vst3; })) {
                        soundfont = text(message,"soundfont",4096); absolute(soundfont);
                    }
                    live = std::make_unique<Live>(std::move(plan), soundfont, master, scale, autoplay);
                } else {
                    if (type == "load") throw std::runtime_error("load already applied; start a new helper session");
                    live->apply(type, message);
                }
                envelope = std::move(next); // acknowledge only after safe graph application
                auto ack = envelope.message("ack"); ack["operation"] = type;
                ack["part_id"] = message.value("part_id",Json(nullptr)); wire.send(ack);
                next_status = std::chrono::steady_clock::now();
            }
            auto now = std::chrono::steady_clock::now();
            if (live && now >= next_status) {
                wire.send(live->status(envelope));
                next_status = now + std::chrono::milliseconds(40);
            }
        }
        return 0;
    } catch (const std::exception& error) {
        live.reset(); // silence/cleanup before reporting fatal control failure
        if (!envelope.session.empty()) {
            try {
                auto j = envelope.message("error");
                j["message"] = std::string(error.what()).substr(0, 1024);
                wire.send(j);
            } catch (...) {}
        }
        throw;
    }
}
} // namespace rms
