#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace rms {
constexpr uint64_t max_tick = 82944000;
constexpr size_t max_items = 250000;
struct Tempo { uint64_t tick; double bpm; };
struct Note { uint64_t start_tick, duration_ticks; uint8_t pitch, velocity; };
struct StateFile { std::string path, sha256; uint64_t bytes = 0; };
struct Source {
    bool vst3 = false;
    std::string path, class_id, module_sha256;
    double gain = 1;
    StateFile component, controller;
};
struct Part {
    std::string id;
    bool enabled = true;
    uint8_t volume = 100, program = 0;
    std::vector<Note> notes;
    Source source{};
};
struct Plan {
    uint64_t duration_ticks = 0;
    std::vector<Tempo> tempos;
    std::vector<Part> parts;
    void validate() const;
};
class Timeline {
public:
    explicit Timeline(const Plan&);
    double seconds(uint64_t tick) const noexcept;
    uint64_t tick(double seconds) const noexcept;
    double quarters(double seconds) const noexcept;
    double bpm(double seconds) const noexcept;
    double next_tempo(double seconds) const noexcept;
private:
    struct Point { uint64_t tick; double seconds, seconds_per_tick; };
    std::vector<Point> points_;
};
// All methods on Instrument are called only from the callback or while quiesced.
// Implementations must prepare all host buffers before activation.
struct Instrument {
    virtual ~Instrument() = default;
    virtual void note_on(uint8_t pitch, uint8_t velocity) noexcept = 0;
    virtual void note_off(uint8_t pitch) noexcept = 0;
    virtual void silence() noexcept = 0;
    virtual void configure(uint8_t volume, uint8_t program) noexcept = 0;
    virtual void render(float* left, float* right, uint32_t frames) noexcept = 0;
    virtual bool failed() const noexcept { return false; }
    virtual uint32_t latency() const noexcept { return 0; }
    virtual void transport(double, double, bool, double = 0, uint64_t = 0) noexcept {}
};
enum class State { stopped, playing, paused, finished };
const char* state_name(State) noexcept;
class Engine {
public:
    // Instruments are externally owned, outlive Engine and match plan part order.
    Engine(Plan, uint32_t sample_rate, uint32_t capacity,
           const std::vector<Instrument*>& instruments);
    void play() noexcept;
    void pause() noexcept;
    void stop() noexcept;
    void seek(double real_delta) noexcept;
    void tempo(double scale) noexcept;
    void mix(size_t part, bool enabled, uint8_t volume, uint8_t program) noexcept;
    void master(double percent) noexcept;
    void gain(size_t part, double gain) noexcept;
    uint32_t plugin_latency() const noexcept { return plugin_latency_; }
    void rebuild_latency();
    // Device frames emitted as silence while the control thread owns the graph.
    // Unlike an xrun these advance the device clock, not musical position.
    void hold(uint64_t frames) noexcept;
    void advance_gap(uint64_t frames) noexcept;
    void process(float* left, float* right, uint32_t frames, uint64_t latency) noexcept;
    State state() const noexcept { return state_; }
    double position() const noexcept { return position_; }
    double audible() const noexcept { return audible_; }
    double duration() const noexcept { return duration_; }
    double scale() const noexcept { return scale_; }
    uint64_t score_tick() const noexcept { return timeline_.tick(audible_); }
    bool mapping_valid() const noexcept { return mapping_valid_; }
private:
    struct Event { double seconds; uint8_t part, pitch, velocity; bool on; };
    struct History { uint64_t frame; double position, scale; };
    Plan plan_;
    Timeline timeline_;
    std::vector<Event> events_;
    std::vector<Instrument*> instruments_;
    std::vector<float> scratch_l_, scratch_r_;
    struct Delay { std::vector<float> left, right; size_t cursor = 0; };
    std::vector<Delay> delays_;
    std::vector<double> part_gains_;
    uint32_t plugin_latency_ = 0;
    uint32_t rate_, capacity_;
    size_t cursor_ = 0;
    State state_ = State::stopped;
    double position_ = 0, audible_ = 0, duration_, scale_ = 1;
    double gain_ = 0, gain_target_ = 1, gain_step_;
    uint64_t clock_ = 0, history_floor_ = 0;
    // Retain slope changes over a ten-second device horizon, not one entry per
    // audio callback. Capacity exhaustion is fatal rather than approximate.
    static constexpr size_t history_capacity_ = 1024;
    History history_[history_capacity_]{};
    size_t history_count_ = 0, history_next_ = 0;
    bool mapping_valid_ = true;
    bool history(uint64_t frame, double position, double scale) noexcept;
    void update_audible(uint64_t latency) noexcept;
    void silence() noexcept;
    void relocate(double position, bool reset_history) noexcept;
    void chase(size_t part) noexcept;
    void dispatch() noexcept;
    void render(float*, float*, uint32_t offset, uint32_t count) noexcept;
};
} // namespace rms
