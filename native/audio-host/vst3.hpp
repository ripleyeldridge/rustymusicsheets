#pragma once
#include "protocol.hpp"
#include <memory>

namespace rms {
Json plugin_capabilities();
bool plugin_editor_callback_active() noexcept;
Json build_info();
Json scan_plugin(const Json&);
std::string module_digest(const std::string& bundle);
class Vst3 final : public Instrument {
public:
    Vst3(const Source&, uint32_t rate, uint32_t capacity);
    ~Vst3() override;
    void note_on(uint8_t, uint8_t) noexcept override;
    void note_off(uint8_t) noexcept override;
    void silence() noexcept override;
    void configure(uint8_t, uint8_t) noexcept override {}
    void render(float*, float*, uint32_t) noexcept override;
    bool failed() const noexcept override;
    uint32_t latency() const noexcept override;
    void transport(double, double, bool, double = 0, uint64_t = 0) noexcept override;
    Json parameters(uint32_t offset, uint32_t limit);
    void parameter(uint32_t id, double value);
    std::pair<std::string, std::string> capture();
    void preset(const std::string&);
    bool dirty() const noexcept;
    void clean() noexcept;
    void editor(bool open, const std::string& label = "RustyMusicSheets Instrument Editor");
    uint64_t editor_window() const noexcept;
    bool pump();
private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
}
