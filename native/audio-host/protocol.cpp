#include "protocol.hpp"

#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <cmath>
#include <cstring>
#include <fcntl.h>
#include <openssl/evp.h>
#include <poll.h>
#include <stdexcept>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

namespace rms {
namespace {
constexpr size_t max_frame = 1024 * 1024, max_plan = 64 * 1024 * 1024;
[[noreturn]] void fail(const char* message) { throw std::runtime_error(message); }
int64_t now_ms() {
    return std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::steady_clock::now().time_since_epoch()).count();
}
void envelope(const Json& j) {
    if (!j.is_object() || integer(j, "version") != 2) fail("unsupported protocol version; reader/helper both require protocol 2");
    hex64(text(j, "session", 64));
    integer(j, "request_id");
    integer(j, "epoch");
    text(j, "type", 32);
}
} // namespace
void absolute(const std::string& path) {
    if (path.empty() || path[0] != '/' || path.find('\0') != std::string::npos)
        fail("absolute path without NUL required");
}
std::string digest_bytes(const std::string& contents) {
    unsigned char hash[EVP_MAX_MD_SIZE];
    unsigned int size = 0;
    if (EVP_Digest(contents.data(), contents.size(), hash, &size, EVP_sha256(), nullptr) != 1 || size != 32)
        fail("SHA256 computation failed");
    static const char hex[] = "0123456789abcdef";
    std::string result;
    for (unsigned i = 0; i < size; ++i) { result += hex[hash[i] >> 4]; result += hex[hash[i] & 15]; }
    return result;
}
std::string read_private(const StateFile& file, uint64_t maximum) {
    absolute(file.path);
    hex64(file.sha256);
    if (file.bytes > maximum) fail("state exceeds size limit");
    int fd = open(file.path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (fd < 0) fail("cannot open private state file");
    struct Guard { int fd; ~Guard() { close(fd); } } guard{fd};
    struct stat st{};
    if (fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_uid != getuid() ||
        (st.st_mode & 0077) || st.st_nlink != 1 || st.st_size < 0 ||
        static_cast<uint64_t>(st.st_size) != file.bytes) fail("state must be private owned regular file");
    std::string data(static_cast<size_t>(file.bytes), '\0');
    size_t offset = 0;
    while (offset < data.size()) {
        auto n = read(fd, data.data() + offset, data.size() - offset);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) fail("short state read");
        offset += static_cast<size_t>(n);
    }
    char extra;
    if (read(fd, &extra, 1) != 0 || digest_bytes(data) != file.sha256) fail("state changed or digest mismatch");
    return data;
}
StateFile write_private(const std::string& path, const std::string& data) {
    absolute(path);
    int fd = open(path.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (fd < 0) fail("cannot create fresh state file");
    struct Guard { int fd; ~Guard() { close(fd); } } guard{fd};
    size_t offset = 0;
    while (offset < data.size()) {
        auto n = write(fd, data.data() + offset, data.size() - offset);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) fail("state write failed");
        offset += static_cast<size_t>(n);
    }
    if (fsync(fd)) fail("state sync failed");
    return {path, digest_bytes(data), data.size()};
}
uint64_t integer(const Json& j, const char* key, uint64_t maximum) {
    const auto& v = j.at(key);
    if (!v.is_number_unsigned() && !(v.is_number_integer() && v.get<int64_t>() >= 0))
        fail("expected unsigned integer");
    uint64_t n = v.get<uint64_t>();
    if (n > maximum) fail("integer field exceeds bound");
    return n;
}
double number(const Json& j, const char* key, double low, double high) {
    const auto& v = j.at(key);
    if (!v.is_number()) fail("expected finite number");
    double n = v.get<double>();
    if (!std::isfinite(n) || n < low || n > high) fail("number field exceeds bound");
    return n;
}
bool boolean(const Json& j, const char* key) {
    if (!j.at(key).is_boolean()) fail("expected boolean");
    return j.at(key).get<bool>();
}
std::string text(const Json& j, const char* key, size_t maximum) {
    if (!j.at(key).is_string()) fail("expected string");
    auto s = j.at(key).get<std::string>();
    if (s.empty() || s.size() > maximum || s.find('\0') != std::string::npos)
        fail("empty or oversized/NUL string");
    return s;
}
void hex64(const std::string& s) {
    if (s.size() != 64 || !std::all_of(s.begin(), s.end(), [](char c) {
            return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f');
        })) fail("expected lowercase SHA256/session hex");
}
Json parse_json(const std::string& input) {
    // Bound nesting and reject duplicate keys, even for unknown extension objects.
    std::vector<std::vector<std::string>> keys;
    return Json::parse(input, [&keys](int depth, Json::parse_event_t event, Json& value) {
        if (depth > 32) fail("JSON nesting exceeds limit");
        if (event == Json::parse_event_t::object_start) keys.emplace_back();
        if (event == Json::parse_event_t::key) {
            auto key = value.get<std::string>();
            if (key.size() > 256) fail("JSON key exceeds limit");
            auto& current = keys.back();
            if (current.size() >= 64 || std::find(current.begin(), current.end(), key) != current.end())
                fail("duplicate or excessive object keys");
            current.push_back(std::move(key));
        }
        if (event == Json::parse_event_t::object_end) keys.pop_back();
        return true;
    });
}
Plan parse_plan(const Json& j) {
    if (!j.is_object() || integer(j, "version") != 2 ||
        integer(j, "ticks_per_quarter", 960) != 960) fail("unsupported plan format");
    Plan p;
    p.duration_ticks = integer(j, "duration_ticks", max_tick);
    const auto& tempos = j.at("tempos");
    const auto& parts = j.at("parts");
    if (!tempos.is_array() || tempos.empty() || tempos.size() > max_items ||
        !parts.is_array() || parts.empty() || parts.size() > 32) fail("invalid plan arrays");
    for (const auto& t : tempos) {
        double bpm = number(t, "bpm", 0, 60000);
        if (bpm <= 0) fail("tempo must be positive");
        p.tempos.push_back({integer(t, "tick", max_tick), bpm});
    }
    size_t count = tempos.size();
    for (const auto& part : parts) {
        Part v;
        v.id = text(part, "id", 256);
        v.enabled = boolean(part, "enabled");
        v.volume = static_cast<uint8_t>(integer(part, "volume", 127));
        v.program = static_cast<uint8_t>(integer(part, "program", 127));
        if (part.contains("source")) {
            const auto& source = part.at("source");
            auto kind = text(source, "kind", 32);
            if (kind != "soundfont" && kind != "vst3") fail("unsupported instrument source");
            if (kind == "vst3") {
                v.source.vst3 = true;
                v.source.path = text(source, "path", 4096);
                absolute(v.source.path);
                v.source.class_id = text(source, "class_id", 32);
                if (v.source.class_id.size() != 32 || !std::all_of(v.source.class_id.begin(), v.source.class_id.end(),
                    [](char c) { return (c >= '0' && c <= '9') || (c >= 'A' && c <= 'F'); }))
                    fail("class_id must be 32 uppercase hex digits");
                v.source.module_sha256 = text(source, "module_sha256", 64);
                hex64(v.source.module_sha256);
                v.source.gain = number(source, "gain", 0, 1);
                auto state = [&](const char* key) {
                    StateFile file;
                    const auto& s = source.at(key);
                    if (!s.is_null()) {
                        file = {text(s, "path", 4096), text(s, "sha256", 64), integer(s, "bytes", max_plan)};
                        absolute(file.path); hex64(file.sha256);
                    }
                    return file;
                };
                v.source.component = state("component_state");
                v.source.controller = state("controller_state");
                if (v.source.component.bytes + v.source.controller.bytes > max_plan) fail("combined state exceeds 64MiB");
            }
        }
        const auto& notes = part.at("notes");
        if (!notes.is_array() || notes.size() > max_items - count) fail("invalid/excessive notes");
        count += notes.size();
        v.notes.reserve(notes.size());
        for (const auto& n : notes) {
            const auto velocity = integer(n, "velocity", 127);
            if (!velocity) fail("zero note velocity");
            v.notes.push_back({integer(n, "start_tick", max_tick), integer(n, "duration_ticks", max_tick),
                              static_cast<uint8_t>(integer(n, "pitch", 127)), static_cast<uint8_t>(velocity)});
        }
        std::sort(v.notes.begin(), v.notes.end(), [](auto a, auto b) {
            if (a.start_tick != b.start_tick) return a.start_tick < b.start_tick;
            return a.pitch < b.pitch;
        });
        p.parts.push_back(std::move(v));
    }
    p.validate();
    Timeline timing(p); // independently bound finite, representable duration
    return p;
}
Plan read_plan(const Json& load) {
    auto path = text(load, "plan_path", 4096);
    absolute(path);
    auto digest = text(load, "plan_sha256", 64);
    hex64(digest);
    uint64_t bytes = integer(load, "plan_bytes", max_plan);
    if (!bytes) fail("empty plan file");
    int fd = open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (fd < 0) fail("cannot open private plan file");
    struct Guard { int fd; ~Guard() { close(fd); } } guard{fd};
    struct stat st{};
    if (fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_uid != getuid() ||
        (st.st_mode & 0077) || st.st_nlink != 1 || st.st_size < 0 ||
        static_cast<uint64_t>(st.st_size) != bytes) fail("plan must be private, owned regular file with matching size");
    std::string contents(static_cast<size_t>(bytes), '\0');
    size_t offset = 0;
    while (offset < contents.size()) {
        ssize_t n = read(fd, contents.data() + offset, contents.size() - offset);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) fail("short read of plan");
        offset += static_cast<size_t>(n);
    }
    char extra;
    if (read(fd, &extra, 1) != 0) fail("plan changed during read");
    unsigned char hash[EVP_MAX_MD_SIZE];
    unsigned int hash_size = 0;
    if (EVP_Digest(contents.data(), contents.size(), hash, &hash_size, EVP_sha256(), nullptr) != 1 ||
        hash_size != 32) fail("SHA256 computation failed");
    static const char hex[] = "0123456789abcdef";
    std::string actual;
    for (unsigned i = 0; i < hash_size; ++i) { actual += hex[hash[i] >> 4]; actual += hex[hash[i] & 15]; }
    if (actual != digest) fail("plan SHA256 mismatch");
    return parse_plan(parse_json(contents));
}
void Envelope::hello(const Json& j) {
    envelope(j);
    if (text(j, "type", 32) != "hello" || integer(j, "request_id") || integer(j, "epoch"))
        fail("expected initial hello at request/epoch zero");
    session = text(j, "session", 64);
}
std::string Envelope::command(const Json& j) {
    envelope(j);
    if (text(j, "session", 64) != session) fail("session mismatch");
    auto id = integer(j, "request_id"), ep = integer(j, "epoch");
    if (id <= request || ep < epoch) fail("stale request or epoch");
    auto type = text(j, "type", 32);
    bool transport = type == "play" || type == "pause" || type == "stop" || type == "seek" || type == "tempo";
    if (transport ? ep <= epoch : ep != epoch) fail("invalid command epoch");
    if (type != "load" && type != "play" && type != "pause" && type != "stop" && type != "seek" &&
        type != "tempo" && type != "mix" && type != "master_volume" && type != "shutdown" &&
        type != "scan" && type != "gain" && type != "parameters" && type != "parameter" &&
        type != "editor_open" && type != "editor_close" && type != "preset_load" && type != "checkpoint") fail("unknown command");
    // The caller invokes this on a copy and commits only after safe application.
    request = id;
    epoch = ep;
    return type;
}
Json Envelope::message(const char* type) const {
    return {{"version", 2}, {"session", session}, {"request_id", request}, {"epoch", epoch}, {"type", type}};
}
Wire::Wire(const std::string& path, pid_t parent) {
    absolute(path);
    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    if (path.size() >= sizeof(address.sun_path)) fail("socket path too long");
    auto slash = path.rfind('/');
    auto directory = path.substr(0, slash ? slash : 1);
    struct stat st{};
    if (lstat(directory.c_str(), &st) || !S_ISDIR(st.st_mode) || st.st_uid != getuid() || (st.st_mode & 0077))
        fail("socket directory must be private and owned by current uid");
    std::memcpy(address.sun_path, path.c_str(), path.size() + 1);
    fd_ = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd_ < 0) fail("socket creation failed");
    try {
        if (connect(fd_, reinterpret_cast<sockaddr*>(&address), sizeof(address)) < 0) {
            if (errno != EINPROGRESS) fail("cannot connect parent socket");
            pollfd p{fd_, POLLOUT, 0};
            if (poll(&p, 1, 3000) <= 0) fail("parent connect timeout");
            int error = 0;
            socklen_t size = sizeof(error);
            if (getsockopt(fd_, SOL_SOCKET, SO_ERROR, &error, &size) || error) fail("parent connection failed");
        }
        ucred credentials{};
        socklen_t size = sizeof(credentials);
        if (getsockopt(fd_, SOL_SOCKET, SO_PEERCRED, &credentials, &size) ||
            credentials.uid != getuid() || credentials.pid != parent || parent != getppid())
            fail("parent peer credentials mismatch");
        input_.reserve(max_frame + 4);
    } catch (...) { close(fd_); fd_ = -1; throw; }
}
Wire::~Wire() { if (fd_ >= 0) close(fd_); }
bool Wire::receive(Json& message, int timeout) {
    if (partial_since_ && now_ms() - partial_since_ > 3000) fail("partial control frame timeout");
    pollfd p{fd_, POLLIN, 0};
    int ready = poll(&p, 1, timeout);
    if (ready < 0 && errno == EINTR) return false;
    if (ready < 0 || (p.revents & (POLLERR | POLLNVAL))) fail("control socket poll failed");
    if (!ready) return false;
    std::array<uint8_t, 16384> buffer{};
    // Read only one frame, avoiding a peer flooding unbounded queued input.
    size_t remaining = input_.size() < 4 ? 4 - input_.size() : wanted_ + 4 - input_.size();
    ssize_t n = recv(fd_, buffer.data(), std::min(remaining, buffer.size()), 0);
    if (n < 0 && (errno == EAGAIN || errno == EINTR)) return false;
    if (n <= 0) fail("parent disconnected");
    if (input_.empty()) partial_since_ = now_ms();
    input_.insert(input_.end(), buffer.begin(), buffer.begin() + n);
    if (input_.size() == 4) {
        wanted_ = (uint32_t(input_[0]) << 24) | (uint32_t(input_[1]) << 16) |
                  (uint32_t(input_[2]) << 8) | input_[3];
        if (!wanted_ || wanted_ > max_frame) fail("control frame exceeds 1MiB");
    }
    if (wanted_ && input_.size() == wanted_ + 4) {
        message = parse_json(std::string(input_.begin() + 4, input_.end()));
        input_.clear();
        wanted_ = 0;
        partial_since_ = 0;
        return true;
    }
    return false;
}
void Wire::send(const Json& message) {
    auto contents = message.dump();
    if (contents.empty() || contents.size() > max_frame) fail("outgoing frame exceeds bound");
    uint32_t n = static_cast<uint32_t>(contents.size());
    std::string frame;
    frame.push_back(static_cast<char>(n >> 24)); frame.push_back(static_cast<char>(n >> 16));
    frame.push_back(static_cast<char>(n >> 8)); frame.push_back(static_cast<char>(n));
    frame += contents;
    size_t offset = 0;
    auto deadline = now_ms() + 250;
    while (offset < frame.size()) {
        ssize_t sent = ::send(fd_, frame.data() + offset, frame.size() - offset, MSG_NOSIGNAL);
        if (sent > 0) { offset += static_cast<size_t>(sent); continue; }
        if (sent < 0 && errno == EINTR) continue;
        if (sent < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            pollfd p{fd_, POLLOUT, 0};
            int remaining = static_cast<int>(deadline - now_ms());
            if (remaining > 0 && poll(&p, 1, remaining) > 0) continue;
        }
        fail("parent not reading control socket");
    }
}
} // namespace rms
