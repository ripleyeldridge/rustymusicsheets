#pragma once
#include "protocol.hpp"
#include "vst3.hpp"
#include <atomic>
#include <fluidsynth.h>
#include <jack/jack.h>
#include <memory>

namespace rms {
class SoundFont final : public Instrument {
public:
    SoundFont(const std::string& path, uint32_t rate);
    ~SoundFont() override;
    void note_on(uint8_t, uint8_t) noexcept override;
    void note_off(uint8_t) noexcept override;
    void silence() noexcept override;
    void configure(uint8_t, uint8_t) noexcept override;
    void render(float*, float*, uint32_t) noexcept override;
    bool failed() const noexcept { return failed_.load(); }
private:
    fluid_settings_t* settings_ = nullptr;
    fluid_synth_t* synth_ = nullptr;
    int program_ = -1;
    std::atomic<bool> failed_{false};
};
class Live {
public:
    Live(Plan, const std::string& soundfont, double master, double scale, bool autoplay);
    ~Live();
    void apply(const std::string& type, const Json&);
    Json operation(const std::string& type, const Json&);
    Json status(const Envelope&);
private:
    static constexpr uint32_t capacity = 65536;
    jack_client_t* client_ = nullptr;
    jack_port_t *left_ = nullptr, *right_ = nullptr;
    uint32_t rate_ = 0;
    jack_nframes_t expected_frame_ = 0;
    bool clock_started_ = false;
    Plan plan_;
    std::vector<std::unique_ptr<Instrument>> nodes_;
    std::unique_ptr<Engine> engine_;
    std::atomic<bool> quiescent_{true}, in_callback_{false}, lost_{false};
    std::atomic<bool> control_hold_{false}, mapping_failed_{false};
    std::atomic<uint64_t> pending_hold_frames_{0};
    std::atomic<uint64_t> xruns_{0}, latency_{0};
    std::atomic<uint64_t> telemetry_sequence_{0};
    std::atomic<double> position_{0}, rendered_{0}, duration_{0}, scale_{1};
    std::atomic<uint64_t> tick_{0};
    std::atomic<State> state_{State::stopped};
    static int process(jack_nframes_t, void*) noexcept;
    static void shutdown(void*) noexcept;
    static int rate_changed(jack_nframes_t, void*) noexcept;
    static int xrun(void*) noexcept;
    void publish() noexcept;
    void quiesce();
    void cleanup() noexcept;
};
int serve(const std::string&, pid_t, const std::string& mode = "serve");
} // namespace rms
