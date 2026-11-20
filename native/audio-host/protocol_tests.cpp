#include "protocol.hpp"
#include <atomic>
#include <chrono>
#include <cerrno>
#include <cstring>
#include <cstdio>
#include <functional>
#include <openssl/evp.h>
#include <poll.h>
#include <signal.h>
#include <stdexcept>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <thread>
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
    check(!envelope.editor_keepalive,"legacy clients do not enable editor keepalives");
    auto negotiated = hello; negotiated["client_capabilities"] = {"editor_keepalive"};
    envelope.hello(negotiated);
    check(envelope.editor_keepalive,"explicit editor keepalive negotiation");
    negotiated["client_capabilities"] = "editor_keepalive";
    rejected([&] { envelope.hello(negotiated); });
    negotiated["client_capabilities"] = {42};
    rejected([&] { envelope.hello(negotiated); });
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
void wire_tests() {
    char directory[] = "/tmp/rms-wire-XXXXXX";
    check(mkdtemp(directory),"private wire test directory");
    std::string path = std::string(directory)+"/control";
    int listener = socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0);
    check(listener>=0,"wire listener");
    sockaddr_un address{}; address.sun_family=AF_UNIX;
    std::memcpy(address.sun_path,path.c_str(),path.size()+1);
    check(bind(listener,reinterpret_cast<sockaddr*>(&address),sizeof(address))==0 &&
        listen(listener,1)==0,"bind wire listener");
    pid_t child = fork();
    check(child>=0,"fork wire client");
    if (!child) {
        close(listener);
        try {
            rms::Wire wire(path,getppid());
            std::atomic<bool> failed{false};
            auto writer = [&](int id) {
                try {
                    for (int i=0;i<4;++i)
                        wire.send({{"writer",id},{"index",i},{"payload",std::string(300000,char('a'+id))}});
                } catch (...) { failed=true; }
            };
            std::thread a(writer,0),b(writer,1); a.join(); b.join();
            check(!failed,"concurrent wire writers");
            rms::Json message;
            check(!wire.receive(message,2000),"read only buffered test header");
            wire.send({{"header",1}});
            std::this_thread::sleep_for(std::chrono::milliseconds(3100));
            check(wire.receive(message,0) && message["buffered"]==true,
                "complete buffered frame survives a blocked UI reader");
            wire.send({{"buffered_read",true}});
            check(!wire.receive(message,2000),"read only stalled test header");
            wire.send({{"header",2}});
            std::this_thread::sleep_for(std::chrono::milliseconds(3100));
            bool timed_out=false;
            try { wire.receive(message,0); }
            catch (const std::exception& error) {
                timed_out=std::string(error.what()).find("partial control frame timeout")!=std::string::npos;
            }
            check(timed_out,"unbuffered partial frames still time out");
            _exit(0);
        } catch (...) { _exit(2); }
    }
    int peer=-1;
    bool reaped=false;
    auto cleanup = [&] {
        if (peer>=0) close(peer);
        close(listener); unlink(path.c_str()); rmdir(directory);
    };
    try {
        pollfd ready{listener,POLLIN,0};
        check(poll(&ready,1,5000)>0,"wire child connect deadline");
        peer=accept4(listener,nullptr,nullptr,SOCK_CLOEXEC);
        check(peer>=0,"accept wire client");
        timeval timeout{5,0};
        check(setsockopt(peer,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof(timeout))==0,"wire read timeout");
        auto exact = [&](char* data,size_t bytes) {
            size_t read=0;
            while (read<bytes) {
                auto n=recv(peer,data+read,bytes-read,0);
                if (n<0 && errno==EINTR) continue;
                check(n>0,"read complete wire frame");
                read+=static_cast<size_t>(n);
            }
        };
        auto receive = [&] {
            unsigned char header[4]{};
            exact(reinterpret_cast<char*>(header),4);
            uint32_t size=(uint32_t(header[0])<<24)|(uint32_t(header[1])<<16)|(uint32_t(header[2])<<8)|header[3];
            check(size>0 && size<=1024*1024,"bounded concurrent frame");
            std::string body(size,'\0'); exact(body.data(),body.size());
            return rms::parse_json(body);
        };
        const std::string body=R"({"buffered":true})";
        auto header = [&] {
            uint32_t size=static_cast<uint32_t>(body.size());
            unsigned char bytes[4]{static_cast<unsigned char>(size>>24),static_cast<unsigned char>(size>>16),
                static_cast<unsigned char>(size>>8),static_cast<unsigned char>(size)};
            check(send(peer,bytes,4,MSG_NOSIGNAL)==4,"send partial test header");
        };
        header();
        unsigned counts[2]{};
        for (int i=0;i<8;++i) {
            auto value=receive(); int writer=value["writer"];
            check(writer==0 || writer==1,"concurrent writer identity");
            check(value["index"]==counts[writer]++,"per-writer frame order");
            check(value["payload"]==std::string(300000,char('a'+writer)),"frames cannot interleave");
        }
        check(receive()["header"]==1,"buffered header read acknowledgement");
        check(send(peer,body.data(),body.size(),MSG_NOSIGNAL)==static_cast<ssize_t>(body.size()),
            "buffer control body during modal wait");
        check(receive()["buffered_read"]==true,"buffered frame accepted");
        header(); check(receive()["header"]==2,"stalled header read acknowledgement");
        int status=0;
        auto waited=waitpid(child,&status,0);
        reaped=waited==child;
        check(reaped && WIFEXITED(status) && WEXITSTATUS(status)==0,
            "concurrent/buffered/stalled wire client passed");
        cleanup();
    } catch (...) {
        if (!reaped) { kill(child,SIGKILL); waitpid(child,nullptr,0); }
        cleanup(); throw;
    }
}
}
int main() {
    try {
        tests();
        wire_tests();
        std::puts("protocol: envelopes, ordering/epochs, strict JSON, plan bounds, digest/private file/symlink rejection passed");
    } catch (const std::exception& e) {
        std::fprintf(stderr, "protocol test: %s\n", e.what()); return 1;
    }
}
