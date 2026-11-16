#include "live.hpp"
#include <cstdio>
#include <cstdlib>
#include <stdexcept>

// Complete link-time JACK double for this test target only; no server contact,
// synth loading, audio activation or package installation occurs.
struct _jack_client { int placeholder; };
struct _jack_port { int placeholder; };
namespace {
_jack_client client{};
_jack_port port{};
bool unavailable = false, closed = false, no_autostart = false, correct_flags = false;
}
extern "C" {
jack_client_t* jack_client_open(const char*, jack_options_t flags, jack_status_t*, ...) {
    no_autostart = (flags & JackNoStartServer) != 0;
    return unavailable ? nullptr : &client;
}
int jack_client_close(jack_client_t*) { closed = true; return 0; }
int jack_deactivate(jack_client_t*) { return 0; }
jack_nframes_t jack_get_sample_rate(jack_client_t*) { return 48000; }
jack_nframes_t jack_get_buffer_size(jack_client_t*) { return 256; }
jack_port_t* jack_port_register(jack_client_t*, const char*, const char*, unsigned long, unsigned long) { return &port; }
const char** jack_get_ports(jack_client_t*, const char*, const char*, unsigned long flags) {
    correct_flags = flags == (JackPortIsPhysical | JackPortIsTerminal | JackPortIsInput);
    return nullptr;
}
void jack_free(void*) { std::abort(); }
int jack_activate(jack_client_t*) { std::abort(); }
int jack_connect(jack_client_t*, const char*, const char*) { std::abort(); }
const char* jack_port_name(const jack_port_t*) { std::abort(); }
void jack_port_get_latency_range(jack_port_t*, jack_latency_callback_mode_t, jack_latency_range_t*) { std::abort(); }
int jack_port_connected(const jack_port_t*) { std::abort(); }
void* jack_port_get_buffer(jack_port_t*, jack_nframes_t) { std::abort(); }
jack_nframes_t jack_last_frame_time(const jack_client_t*) { std::abort(); }
int jack_set_process_callback(jack_client_t*, JackProcessCallback, void*) { std::abort(); }
int jack_set_sample_rate_callback(jack_client_t*, JackSampleRateCallback, void*) { std::abort(); }
int jack_set_xrun_callback(jack_client_t*, JackXRunCallback, void*) { std::abort(); }
void jack_on_shutdown(jack_client_t*, JackShutdownCallback, void*) { std::abort(); }
}
int main() {
    try {
        rms::Plan p;
        p.duration_ticks = 960;
        p.tempos = {{0, 120}};
        p.parts.push_back({"part", true, 100, 0, {{0, 960, 60, 100}}});
        bool rejected = false;
        try { rms::Live live(p, "/not-loaded.sf2", 100, 1, false); }
        catch (const std::exception& e) {
            rejected = std::string(e.what()).find("physical terminal") != std::string::npos;
        }
        if (!rejected || !closed || !no_autostart || !correct_flags)
            throw std::runtime_error("no physical outputs must fail helpfully before synthesis/activation");
        unavailable = true;
        rejected = false;
        try { rms::Live live(p, "/not-loaded.sf2", 100, 1, false); }
        catch (const std::exception& e) {
            rejected = std::string(e.what()).find("JACK unavailable") != std::string::npos;
        }
        if (!rejected) throw std::runtime_error("absent JACK server must fail helpfully");
        std::puts("live failure tests: no server and zero physical playback ports fail without starting audio");
    } catch (const std::exception& e) {
        std::fprintf(stderr, "live failure test: %s\n", e.what()); return 1;
    }
}
