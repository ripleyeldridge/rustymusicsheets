#pragma once
#include "core.hpp"
#include <nlohmann/json.hpp>
#include <sys/types.h>

namespace rms {
using Json = nlohmann::json;
uint64_t integer(const Json&, const char* key, uint64_t maximum = UINT64_MAX);
double number(const Json&, const char* key, double minimum, double maximum);
bool boolean(const Json&, const char* key);
std::string text(const Json&, const char* key, size_t maximum);
void hex64(const std::string&);
Plan parse_plan(const Json&);
Plan read_plan(const Json& load);
Json parse_json(const std::string&);
struct Envelope {
    std::string session;
    uint64_t request = 0, epoch = 0;
    void hello(const Json&);
    std::string command(const Json&);
    Json message(const char* type) const;
};
// Off-RT only: partial input and output are bounded and always polled with a deadline.
class Wire {
public:
    Wire(const std::string& path, pid_t parent);
    ~Wire();
    Wire(const Wire&) = delete;
    Wire& operator=(const Wire&) = delete;
    // false means no complete frame yet; EOF/malformed/timeout throws.
    bool receive(Json&, int timeout_ms);
    void send(const Json&);
private:
    int fd_ = -1;
    std::vector<uint8_t> input_;
    size_t wanted_ = 0;
    int64_t partial_since_ = 0;
};
} // namespace rms
