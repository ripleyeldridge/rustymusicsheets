#include "live.hpp"
#include <charconv>
#include <cstdio>
#include <limits>
#include <stdexcept>
#include <string>

int main(int argc, char** argv) {
    try {
        if (argc != 6 || std::string(argv[1]) != "serve" || std::string(argv[2]) != "--socket" ||
            std::string(argv[4]) != "--parent-pid")
            throw std::runtime_error("usage: rms-audio-host serve --socket ABS_PATH --parent-pid DECIMAL_PID");
        std::string s(argv[5]);
        unsigned long long pid = 0;
        auto parsed = std::from_chars(s.data(), s.data() + s.size(), pid);
        if (parsed.ec != std::errc{} || parsed.ptr != s.data() + s.size() || pid <= 1 ||
            pid > static_cast<unsigned long long>(std::numeric_limits<pid_t>::max()))
            throw std::runtime_error("invalid parent PID");
        return rms::serve(argv[3], static_cast<pid_t>(pid));
    } catch (const std::exception& error) {
        std::fprintf(stderr, "rms-audio-host: %.1024s\n", error.what());
        return 1;
    }
}
