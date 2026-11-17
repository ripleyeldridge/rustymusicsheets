#include "live.hpp"
#include <charconv>
#include <cstdio>
#include <limits>
#include <stdexcept>
#include <string>
#include <csignal>
#include <unistd.h>
#ifdef __linux__
#include <sys/prctl.h>
#endif

int main(int argc, char** argv) {
    try {
        if (argc == 2 && std::string(argv[1]) == "--build-info") {
            std::puts(rms::build_info().dump().c_str()); return 0;
        }
        if (argc != 6 || (std::string(argv[1]) != "serve" && std::string(argv[1]) != "probe" &&
            std::string(argv[1]) != "scan") || std::string(argv[2]) != "--socket" ||
            std::string(argv[4]) != "--parent-pid")
            throw std::runtime_error("usage: rms-audio-host serve --socket ABS_PATH --parent-pid DECIMAL_PID");
        std::string s(argv[5]);
        unsigned long long pid = 0;
        auto parsed = std::from_chars(s.data(), s.data() + s.size(), pid);
        if (parsed.ec != std::errc{} || parsed.ptr != s.data() + s.size() || pid <= 1 ||
            pid > static_cast<unsigned long long>(std::numeric_limits<pid_t>::max()))
            throw std::runtime_error("invalid parent PID");
#ifdef __linux__
        if (prctl(PR_SET_PDEATHSIG,SIGKILL) != 0)
            throw std::runtime_error("audio helper parent-death protection failed");
        // Wire verifies getppid()/SO_PEERCRED before any handshake or module load.
#endif
        return rms::serve(argv[3], static_cast<pid_t>(pid), argv[1]);
    } catch (const std::exception& error) {
        std::fprintf(stderr, "rms-audio-host: %.1024s\n", error.what());
        return 1;
    }
}
