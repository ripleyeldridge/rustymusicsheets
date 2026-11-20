#include "core.hpp"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <new>
#include <stdexcept>

namespace {
bool callback = false;
}
void* operator new(std::size_t bytes) {
    if (callback) std::abort();
    if (void* p = std::malloc(bytes ? bytes : 1)) return p;
    throw std::bad_alloc();
}
void* operator new[](std::size_t bytes) { return ::operator new(bytes); }
void operator delete(void* p) noexcept { std::free(p); }
void operator delete[](void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }
void operator delete[](void* p, std::size_t) noexcept { std::free(p); }
namespace {
void check(bool condition, const char* message) {
    if (!condition) throw std::runtime_error(message);
}
void near(double actual, double expected, double tolerance, const char* message) {
    check(std::abs(actual - expected) <= tolerance, message);
}
struct Fake final : rms::Instrument {
    struct Event { uint64_t frame; bool on; uint8_t pitch; };
    std::array<Event, 2048> events{};
    size_t count = 0;
    uint64_t frame = 0;
    bool active = false, nonfinite = false;
    uint8_t volume = 0, program = 0;
    unsigned silences = 0;
    struct Context { double seconds, bpm, quarters; uint64_t continuous; };
    std::array<Context,32> contexts{};
    size_t context_count = 0;
    bool record_context = false;
    void transport(double seconds,double bpm,bool,double quarters,uint64_t continuous) noexcept override {
        if (!record_context) return;
        if (context_count == contexts.size()) std::abort();
        contexts[context_count++] = {seconds,bpm,quarters,continuous};
    }
    void note_on(uint8_t p, uint8_t) noexcept override {
        if (count >= events.size()) std::abort();
        events[count++] = {frame, true, p}; active = true;
    }
    void note_off(uint8_t p) noexcept override {
        if (count >= events.size()) std::abort();
        events[count++] = {frame, false, p}; active = false;
    }
    void silence() noexcept override { active = false; ++silences; }
    void configure(uint8_t v, uint8_t p) noexcept override { volume = v; program = p; }
    void render(float* l, float* r, uint32_t n) noexcept override {
        float value = nonfinite ? std::numeric_limits<float>::infinity() : (active ? 0.01f : 0);
        std::fill_n(l, n, value); std::fill_n(r, n, value); frame += n;
    }
};
rms::Plan plan(unsigned parts = 1) {
    rms::Plan p;
    p.duration_ticks = 3840;
    p.tempos = {{0, 120}, {1920, 60}};
    for (unsigned i = 0; i < parts; ++i)
        p.parts.push_back({"part" + std::to_string(i), true, 73, static_cast<uint8_t>(i), {{0, 3840, 60, 100}}});
    return p;
}
void process(rms::Engine& e, uint32_t frames, uint64_t latency = 0) {
    std::array<float, 1024> l{}, r{};
    check(frames <= l.size(), "test capacity");
    callback = true;
    e.process(l.data(), r.data(), frames, latency);
    callback = false;
    for (uint32_t i = 0; i < frames; ++i)
        check(std::isfinite(l[i]) && std::abs(l[i]) <= 1 && std::isfinite(r[i]), "finite output");
}
void timing() {
    auto p = plan();
    rms::Timeline t(p);
    near(t.seconds(1920), 1, 1e-12, "tempo boundary");
    near(t.seconds(3840), 3, 1e-12, "tempo map duration");
    check(t.tick(2) == 2880, "inverse tempo map");
    for (uint64_t tick = 0; tick < 3840; ++tick)
        check(t.tick(t.seconds(tick)) == tick, "tick roundtrip");
    for (uint32_t rate : {44100u, 48000u, 96000u}) {
        for (uint32_t block : {64u, 127u, 256u, 1024u}) {
            for (double scale : {0.1, 1.0, 4.0}) {
                Fake f;
                auto score = plan();
                score.tempos = {{0, 137}, {450, 103.7}};
                score.duration_ticks = 1000;
                score.parts[0].notes.clear();
                for (uint64_t n = 0; n < 100; ++n)
                    score.parts[0].notes.push_back({n * 9, 9, 60, 100});
                rms::Timeline timeline(score);
                rms::Engine engine(score, rate, 1024, {&f});
                engine.tempo(scale);
                engine.play();
                while (engine.state() == rms::State::playing) process(engine, block);
                check(f.count == 200, "all scheduled notes");
                for (size_t i = 0; i < f.count; ++i) {
                    uint64_t tick = (i / 2) * 9 + (i % 2 ? 9 : 0);
                    auto expected = static_cast<uint64_t>(std::ceil(timeline.seconds(tick) * rate / scale - 1e-7));
                    check(f.events[i].frame == expected, "fractional sample conversion must not drift");
                    check(f.events[i].on == (i % 2 == 0), "noteoff before adjacent noteon");
                }
            }
        }
    }
}
void musical_context() {
    auto score = plan();
    score.duration_ticks = 960;
    score.tempos = {{0,120},{480,240}};
    score.parts[0].notes = {{0,960,60,100}};
    Fake f; f.record_context = true;
    rms::Engine engine(score,8000,1024,{&f});
    engine.tempo(2); engine.play(); process(engine,1024);
    check(f.context_count == 2,"split process at tempo even without a note event");
    near(f.contexts[0].bpm,240,1e-12,"scaled initial tempo");
    near(f.contexts[1].bpm,480,1e-12,"scaled new tempo");
    near(f.contexts[1].quarters,.5,1e-12,"musical quarter at tempo boundary");
    near(f.contexts[1].seconds,.125,1e-12,"sample project position uses scaled seconds");
    check(f.contexts[1].continuous == 1000,"continuous clock does not round each event independently");
    engine.tempo(.5); process(engine,64);
    near(f.contexts[2].quarters,.524,1e-10,"fractional musical position is not quantized to ticks");
    check(f.contexts[2].continuous == 1024,"continuous clock survives live tempo changes");
}
void transport() {
    auto p = plan(32);
    std::array<Fake, 32> f;
    std::vector<rms::Instrument*> nodes;
    for (auto& n : f) nodes.push_back(&n);
    rms::Engine e(p, 48000, 1024, nodes);
    e.play();
    process(e, 480);
    for (auto& n : f) check(n.active && n.count == 1 && n.volume == 73, "independent >15-part nodes, legacy volume");
    e.pause();
    for (auto& n : f) check(!n.active, "pause silences held notes");
    e.tempo(2);
    double old = e.position();
    e.seek(0.5);
    near(e.position(), old + 1, 1e-12, "seek real seconds multiplied by scale");
    near(e.audible(), e.position(), 1e-12, "paused seek visible immediately");
    e.play();
    check(f[31].active, "seek/play chase");
    e.mix(31, false, 50, 20);
    check(!f[31].active, "mute silences immediately");
    e.mix(31, true, 51, 21);
    check(f[31].active && f[31].program == 21 && f[31].volume == 51, "unmute/program chase and CC7");
    e.seek(-100);
    near(e.position(), 0, 0, "negative seek clamps");
    e.seek(100);
    near(e.position(), e.duration(), 0, "positive seek clamps");
    process(e, 1024);
    check(e.state() == rms::State::finished, "end finished");
    e.play();
    near(e.position(), 0, 0, "play from finished restarts");
    e.stop();
    near(e.position(), 0, 0, "stop resets");
    for (auto& n : f) check(!n.active, "stop silences");
}
void latency_and_safety() {
    Fake f;
    rms::Engine e(plan(), 48000, 1024, {&f});
    e.play();
    process(e, 480, 960);
    near(e.audible(), 0, 1e-12, "latency priming");
    process(e, 480, 960);
    process(e, 480, 960);
    near(e.audible(), .01, 1e-12, "audible latency mapping");
    e.tempo(2);
    process(e, 480, 960);
    near(e.audible(), .02, 1e-12, "latency crosses prior tempo epoch");
    process(e, 480, 960);
    process(e, 480, 960);
    near(e.audible(), .05, 1e-12, "latency new tempo mapping");
    e.pause();
    e.seek(0.5);
    double target = e.position();
    e.play();
    process(e, 480, 960);
    near(e.audible(), target, 1e-12, "seek latency history invalidated");
    f.nonfinite = true;
    process(e, 480);
    f.nonfinite = false;
    std::array<float, 1024> l{}, r{};
    e.master(0);
    callback = true;
    e.process(l.data(), r.data(), 1024, 0);
    callback = false;
    check(l[0] >= l[100] && std::abs(l[1023]) < 1e-7, "master ramp smoothing");
    e.master(std::numeric_limits<double>::quiet_NaN());
    process(e, 480);
    double before = e.position();
    e.advance_gap(4800);
    near(e.position(), before + .2, 1e-12, "xrun advances shared device clock");
    check(f.active, "held notes chased after xrun");
    Fake muted;
    rms::Engine initial_zero(plan(), 48000, 1024, {&muted});
    initial_zero.master(0);
    initial_zero.play();
    initial_zero.process(l.data(), r.data(), 1024, 0);
    for (auto sample : l) check(sample == 0, "initial master zero must never leak audio");
    auto short_plan = plan();
    short_plan.duration_ticks = 192;
    short_plan.tempos = {{0, 120}};
    short_plan.parts[0].notes[0].duration_ticks = 192;
    Fake drain;
    rms::Engine ending(short_plan, 48000, 1024, {&drain});
    ending.play();
    for (unsigned i = 0; i < 10; ++i) process(ending, 480, 960);
    near(ending.position(), .1, 1e-12, "render reaches end");
    near(ending.audible(), .08, 1e-12, "end still draining device latency");
    check(ending.state() == rms::State::playing, "not finished before audio drains");
    process(ending, 480, 960);
    check(ending.state() == rms::State::playing, "latency drain pending");
    process(ending, 480, 960);
    check(ending.state() == rms::State::finished, "finished once device latency drains");
    near(ending.audible(), .1, 1e-12, "finished position is duration");
}
void validation() {
    auto rejected = [](rms::Plan p) {
        try { rms::Timeline t(p); } catch (const std::exception&) { return; }
        throw std::runtime_error("bad plan accepted");
    };
    auto p = plan(); p.parts[0].notes.push_back({1, 4, 60, 90}); rejected(p);
    p = plan(); p.tempos[0].bpm = 0; rejected(p);
    p = plan(); p.tempos[0].bpm = 1e-300; rejected(p);
    p = plan(); p.tempos[0].bpm = std::numeric_limits<double>::infinity(); rejected(p);
    p = plan(); p.parts[0].id.clear(); rejected(p);
    p = plan(2); p.parts[1].id = p.parts[0].id; rejected(p);
    p = plan(33); rejected(p);
    p = plan(); p.parts[0].notes[0].duration_ticks = UINT64_MAX; rejected(p);
    p = plan(); p.duration_ticks = rms::max_tick + 1; rejected(p);
    p = plan(); p.tempos = {{0, 60000}};
    rms::Timeline fast(p);
    check(fast.seconds(3840) > 0, "high finite tempo representable");
}
void control_history() {
    auto score = plan();
    score.duration_ticks = 192000; // 100 base seconds
    score.tempos = {{0, 120}};
    score.parts[0].notes[0].duration_ticks = score.duration_ticks;
    Fake f;
    rms::Engine e(score, 48000, 1024, {&f});
    e.play();
    for (unsigned n = 0; n < 10; ++n) process(e, 480, 4800);
    near(e.position(), .1, 1e-12, "pre-control musical position");
    e.tempo(2);
    callback = true;
    e.hold(2400); // five zero-output device callbacks during graph ownership
    callback = false;
    near(e.position(), .1, 1e-12, "intentional device gap holds music");
    process(e, 480, 4800); // device .16, audible device .06 (old slope)
    near(e.audible(), .06, 1e-12, "old tempo slope before queued control hold");
    for (unsigned n = 0; n < 4; ++n) process(e, 480, 4800);
    near(e.audible(), .1, 1e-12, "audible reaches beginning of silent control hold");
    for (unsigned n = 0; n < 4; ++n) process(e, 480, 4800);
    near(e.audible(), .1, 1e-12, "audible stays fixed through silent control hold");
    process(e, 480, 4800);
    near(e.audible(), .1, 1e-12, "audible reaches end of control hold");
    process(e, 480, 4800);
    near(e.audible(), .12, 1e-12, "new tempo slope after device-clock hold");
    check(e.mapping_valid(), "control history remains representable");
    // Xruns are deliberately different from control holds.
    double before = e.position();
    callback = true;
    e.advance_gap(2400);
    callback = false;
    near(e.position(), before + .1, 1e-12, "xrun advances musical clock");

    Fake long_f;
    rms::Engine long_e(score, 384000, 1024, {&long_f});
    long_e.play();
    auto seconds = [&](unsigned count) {
        for (unsigned n = 0; n < count * 6000; ++n) process(long_e, 64, 3840000);
        check(long_e.mapping_valid(), "ten-second history at 384kHz/64 frames");
    };
    seconds(4);
    long_e.tempo(2);
    long_e.hold(768000); // two seconds of silent device time, no music advance
    seconds(6); // device 12, audible device 2, still old tempo
    near(long_e.audible(), 2, 1e-9, "long downstream latency retains old slope");
    seconds(2);
    near(long_e.audible(), 4, 1e-9, "long downstream latency reaches hold");
    seconds(1);
    near(long_e.audible(), 4, 1e-9, "long downstream latency within hold");
    seconds(1);
    near(long_e.audible(), 4, 1e-9, "long downstream latency reaches new slope");
    seconds(1);
    near(long_e.audible(), 6, 1e-9, "long downstream latency uses new slope");
    near(long_e.position(), 26, 1e-8, "musical position excludes control hold");
    std::array<float, 1024> l{}, r{};
    long_e.process(l.data(), r.data(), 64, 3840001);
    check(!long_e.mapping_valid() && !long_f.active, "latency over ten seconds fails closed");
    for (unsigned n = 0; n < 64; ++n) check(l[n] == 0 && r[n] == 0, "unsupported latency emits silence");
}
void history_capacity_and_gain() {
    auto score = plan();
    score.duration_ticks = 192000;
    score.tempos = {{0, 120}};
    score.parts[0].notes[0].duration_ticks = score.duration_ticks;
    Fake crowded;
    rms::Engine overflow(score, 384000, 1024, {&crowded});
    overflow.play();
    for (unsigned n = 0; n < 1100 && overflow.mapping_valid(); ++n) {
        overflow.tempo(n % 2 ? 2 : 1);
        process(overflow, 64, 3840000);
    }
    check(!overflow.mapping_valid() && !crowded.active, "rapid history exhaustion fails closed");
    Fake reclaimed;
    rms::Engine retained(score, 48000, 1024, {&reclaimed});
    retained.play();
    for (unsigned n = 0; n < 1500; ++n) {
        callback = true;
        retained.hold(48000);
        callback = false;
        process(retained, 480, 480000);
        check(retained.mapping_valid(), "old history safely reclaimed beyond ten-second horizon");
    }
    score.parts[0].volume = 0;
    Fake gain;
    rms::Engine controls(score, 48000, 1024, {&gain});
    controls.play();
    process(controls, 480);
    auto notes = gain.count;
    auto silences = gain.silences;
    controls.mix(0, true, 80, 0);
    check(gain.count == notes + 1 && gain.silences == silences + 1 && gain.active,
          "legacy CC7 zero-to-positive silences and chases held notes");
    notes = gain.count;
    silences = gain.silences;
    controls.master(0);
    process(controls, 480);
    controls.master(100);
    process(controls, 480);
    check(gain.count == notes && gain.silences == silences && gain.active,
          "master zero-to-positive never retriggers held notes");
}
}
int main() {
    try {
        timing(); musical_context(); transport(); latency_and_safety(); validation();
        control_history(); history_capacity_and_gain();
        std::puts("core: timing (3 rates x 4 blocks x 3 scales), 32 parts, transport/chase, ten-second sparse latency/control holds, bounded history, CC7/master, safety, validation passed");
    } catch (const std::exception& e) {
        std::fprintf(stderr, "core test: %s\n", e.what()); return 1;
    }
}
