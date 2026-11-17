#include "protocol.hpp"
#include <cstdio>
#include <functional>
#include <openssl/evp.h>
#include <stdexcept>
#include <sys/stat.h>
#include <unistd.h>

namespace {
void check(bool ok, const char* why) { if (!ok) throw std::runtime_error(why); }
void rejected(const std::function<void()>& operation) {
    try { operation(); } catch (const std::exception&) { return; }
    throw std::runtime_error("malformed input accepted");
}
void tests() {
    using rms::Json;
    rms::Envelope envelope;
    Json hello = {{"version", 2}, {"session", std::string(64, 'a')}, {"request_id", 0}, {"epoch", 0}, {"type", "hello"}};
    auto old = hello; old["version"] = 1;
    rejected([&] { envelope.hello(old); });
    envelope.hello(hello);
    auto command = hello;
    command["type"] = "load"; command["request_id"] = 1;
    check(envelope.command(command) == "load", "load command");
    rejected([&] { envelope.command(command); });
    command["request_id"] = 2; command["type"] = "play";
    rejected([&] { envelope.command(command); });
    command["epoch"] = 1; envelope.command(command);
    command["request_id"] = 3; command["type"] = "mix"; command["epoch"] = 2;
    rejected([&] { envelope.command(command); });
    command["epoch"] = 1; envelope.command(command);
    command["request_id"] = -1;
    rejected([&] { envelope.command(command); });
    command["request_id"] = 3.5;
    rejected([&] { envelope.command(command); });
    rejected([] { rms::parse_json(R"({"a":1,"a":2})"); });
    rejected([] { rms::parse_json(std::string(40, '[') + "0" + std::string(40, ']')); });
    rejected([] { rms::parse_json(R"({"x":1e9999})"); });
    Json p = rms::parse_json(R"({"version":2,"ticks_per_quarter":960,"duration_ticks":1920,
        "tempos":[{"tick":0,"bpm":120}],
        "parts":[{"id":"piano","enabled":true,"volume":100,"program":0,
        "notes":[{"start_tick":960,"duration_ticks":960,"pitch":60,"velocity":100},
                 {"start_tick":0,"duration_ticks":960,"pitch":60,"velocity":100}]}]})");
    check(rms::parse_plan(p).parts[0].notes[0].start_tick == 0, "off-RT sort");
    auto bad = p; bad["parts"][0]["notes"][0]["velocity"] = 0;
    rejected([&] { rms::parse_plan(bad); });
    bad = p; bad["parts"][0]["notes"][0]["pitch"] = 256;
    rejected([&] { rms::parse_plan(bad); });
    bad = p; bad["parts"][0]["enabled"] = 1;
    rejected([&] { rms::parse_plan(bad); });
    bad = p; bad["parts"][0]["notes"][0]["start_tick"] = 959;
    rejected([&] { rms::parse_plan(bad); });
    char name[] = "/tmp/rms-plan-XXXXXX";
    int fd = mkstemp(name);
    check(fd >= 0, "temporary plan");
    const std::string bytes = p.dump();
    check(write(fd, bytes.data(), bytes.size()) == static_cast<ssize_t>(bytes.size()), "write plan");
    close(fd);
    Json load = {{"plan_path", name}, {"plan_bytes", bytes.size()}, {"plan_sha256", std::string(64, '0')}};
    rejected([&] { rms::read_plan(load); }); // wrong digest
    unsigned char hash[EVP_MAX_MD_SIZE];
    unsigned size = 0;
    check(EVP_Digest(bytes.data(), bytes.size(), hash, &size, EVP_sha256(), nullptr) == 1, "digest");
    std::string digest;
    for (unsigned i = 0; i < size; ++i) {
        digest += "0123456789abcdef"[hash[i] >> 4];
        digest += "0123456789abcdef"[hash[i] & 15];
    }
    load["plan_sha256"] = digest;
    check(rms::read_plan(load).parts[0].id == "piano", "verified private plan load");
    load["plan_bytes"] = bytes.size() - 1;
    rejected([&] { rms::read_plan(load); });
    load["plan_bytes"] = bytes.size();
    chmod(name, 0644);
    rejected([&] { rms::read_plan(load); }); // public permissions
    std::string link = std::string(name) + "-symlink";
    check(symlink(name, link.c_str()) == 0, "plan symlink");
    load["plan_path"] = link;
    rejected([&] { rms::read_plan(load); });
    unlink(link.c_str()); unlink(name);
}
}
int main() {
    try {
        tests();
        std::puts("protocol: envelopes, ordering/epochs, strict JSON, plan bounds, digest/private file/symlink rejection passed");
    } catch (const std::exception& e) {
        std::fprintf(stderr, "protocol test: %s\n", e.what()); return 1;
    }
}
