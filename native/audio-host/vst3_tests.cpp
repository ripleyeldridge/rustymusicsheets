#include "vst3.hpp"
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <functional>
#include <stdexcept>
#include <sys/stat.h>
#include <unistd.h>
#include <chrono>
#include <thread>
#ifdef RMS_EDITOR_TEST
#include <X11/Xlib.h>
#include <X11/keysym.h>
#endif

namespace {
void check(bool ok,const char* why) { if (!ok) throw std::runtime_error(why); }
void rejected(const std::function<void()>& f) {
    try { f(); } catch (const std::exception&) { return; }
    throw std::runtime_error("invalid plugin input accepted");
}
}
int main(int argc,char** argv) {
    try {
        check(argc == 2 || argc == 3,"fixture path required");
        rms::Source s; s.vst3 = true; s.path = argv[1];
        s.module_sha256 = rms::module_digest(s.path);
        auto result = rms::scan_plugin({{"path",s.path},{"module_sha256",s.module_sha256}});
        check(result["classes"].size() == 1,"one deterministic class");
        s.class_id = result["classes"][0]["class_id"];
        check(s.class_id == "6C73687465737431A1023456789ABCDE","stable class identity");
        rms::Vst3 a(s,48000,256), b(s,48000,256);
        if (argc == 3) {
            bool usable = false;
            for (const auto& capability : rms::plugin_capabilities())
                if (capability == "editor_x11") usable = true;
            if (!usable) { std::puts("X11 editor smoke skipped: no usable X11 display"); return 77; }
            a.editor(true);
            bool interactive = std::string(argv[2]) == "--editor-interactive";
            auto end = std::chrono::steady_clock::now()+std::chrono::milliseconds(interactive ? 90000 : 500);
            while (std::chrono::steady_clock::now() < end) { a.pump(); std::this_thread::sleep_for(std::chrono::milliseconds(10)); }
            if (!interactive) {
#ifdef RMS_EDITOR_TEST
                Display* display = XOpenDisplay(nullptr);
                check(display && a.editor_window(), "native editor window must exist");
                Window window = static_cast<Window>(a.editor_window());
                auto pump = [&] {
                    for (int i=0;i<25;++i) { a.pump(); std::this_thread::sleep_for(std::chrono::milliseconds(10)); }
                };
                auto key = [&](KeySym symbol) {
                    XEvent event{}; event.xkey.type = KeyPress; event.xkey.display = display;
                    event.xkey.window = window; event.xkey.root = DefaultRootWindow(display);
                    event.xkey.same_screen = True; event.xkey.keycode = XKeysymToKeycode(display,symbol);
                    check(XSendEvent(display,window,False,KeyPressMask,&event), "send key only to owned fixture window");
                    XFlush(display); pump();
                };
                key(XK_7);
                check(std::abs(a.parameters(0,1)["parameters"][0]["value"].get<double>()-.7)<1e-9,
                    "native editor keyboard updates controller through host handler");
                key(XK_r);
                XWindowAttributes dimensions{};
                check(XGetWindowAttributes(display,window,&dimensions) && dimensions.width == 640 && dimensions.height == 240,
                    "plugin resize request updates native host window");
                XEvent close{}; close.xclient.type = ClientMessage; close.xclient.window = window;
                close.xclient.message_type = XInternAtom(display,"WM_PROTOCOLS",False);
                close.xclient.format = 32; close.xclient.data.l[0] = static_cast<long>(XInternAtom(display,"WM_DELETE_WINDOW",False));
                check(XSendEvent(display,window,False,NoEventMask,&close), "send owned editor close");
                XFlush(display); pump();
                check(a.editor_window() == 0, "WM close detaches only editor");
                XCloseDisplay(display);
#else
                throw std::runtime_error("editor test needs X11 support");
#endif
            }
            a.editor(false); a.editor(true); a.pump(); a.editor(false);
            std::printf("editor final parameter %.3f dirty=%d\n",
                a.parameters(0,1)["parameters"][0]["value"].get<double>(),a.dirty());
            return 0;
        }
        check(a.latency() == 16,"declared latency");
        check(a.parameters(0,128)["parameters"][0]["id"] == 7,"controller parameter enumeration");
        a.parameter(7,.25); b.parameter(7,.75);
        auto paused_state = a.capture();
        check(!paused_state.first.empty(), "paused parameter edit captured without rendering");
        float l[256]{},r[256]{};
        a.transport(0,120,true); a.note_on(60,127); a.render(l,r,256);
        for (int i=0;i<16;++i) check(l[i] == 0,"latency onset silence");
        check(std::abs(l[16] - .0625f) < 1e-6 && l[16] == r[16],"nonzero deterministic stereo DSP");
        b.note_on(60,127); b.render(l,r,256);
        check(std::abs(l[16] - .1875f) < 1e-6,"independent processor state");
        auto state = a.capture(), state_b = b.capture();
        check(paused_state.first == state.first, "paused capture must include current processor parameter");
        check(state.first != state_b.first && state.second != state_b.second,"independent state chunks");
        char directory[] = "/tmp/rms-vst3-state-XXXXXX";
        check(mkdtemp(directory),"private state directory");
        s.component = rms::write_private(std::string(directory)+"/component",state.first);
        s.controller = rms::write_private(std::string(directory)+"/controller",state.second);
        {
            rms::Vst3 restored(s,48000,256);
            check(std::abs(restored.parameters(0,1)["parameters"][0]["value"].get<double>()-.25) < 1e-9,"controller restore");
            restored.note_on(60,127); restored.render(l,r,256);
            check(std::abs(l[16]-.0625f)<1e-6,"processor restore");
        }
        auto bad = s; bad.component.sha256 = std::string(64,'0');
        rejected([&] { rms::Vst3 invalid(bad,48000,256); });
        rejected([&] { a.parameter(999,.1); });
        rejected([&] { a.parameter(7,NAN); });
        bad = s; bad.class_id = std::string(32,'0');
        rejected([&] { rms::Vst3 invalid(bad,48000,256); });
        a.note_off(60); a.render(l,r,256);
        check(l[16] == 0,"note off reaches processor");
        {
            rms::Vst3 peer(s,48000,256), changing(s,48000,256);
            changing.parameter(8,1);
            check(changing.pump() && changing.latency() == 32,"reported latency refreshes outside callback");
            rms::Plan score; score.duration_ticks = 960; score.tempos = {{0,120}};
            score.parts.push_back({"one",true,127,0,{{0,960,60,127}},s});
            score.parts.push_back({"two",true,127,0,{{0,960,60,127}},s});
            rms::Engine graph(score,48000,256,{&peer,&changing});
            check(graph.plugin_latency() == 32,"graph aligns to slowest actual plugin");
            graph.play(); graph.process(l,r,256,32);
            for (int i=0;i<32;++i) check(l[i] == 0,"faster actual plugin receives alignment delay");
            check(l[32] > 0,"aligned actual plugins produce nonzero output together");
            changing.parameter(8,0);
            check(changing.pump() && changing.latency() == 16,"decreased plugin latency refreshes");
            graph.rebuild_latency();
            check(graph.state() == rms::State::paused && graph.plugin_latency() == 16,"reconfiguration pauses and rebuilds graph alignment");
        }
        {
            std::string preset = "VST3";
            auto integer = [&](uint64_t value,unsigned bytes) {
                for (unsigned i=0;i<bytes;++i) preset += static_cast<char>(value>>(i*8));
            };
            integer(1,4); preset += s.class_id; integer(64,8);
            double value = .6;
            preset.append(reinterpret_cast<const char*>(&value),sizeof(value));
            preset.append(reinterpret_cast<const char*>(&value),sizeof(value));
            preset += "List"; integer(2,4);
            preset += "Comp"; integer(48,8); integer(8,8);
            preset += "Cont"; integer(56,8); integer(8,8);
            auto file = rms::write_private(std::string(directory)+"/fixture.vstpreset",preset);
            a.preset(file.path);
            check(std::abs(a.parameters(0,1)["parameters"][0]["value"].get<double>()-.6)<1e-9,"class-matching SDK preset restores controller");
            a.note_on(60,127); a.render(l,r,256);
            check(std::abs(l[16]-.15f)<1e-6,"class-matching SDK preset restores processor");
            preset[8] = '0';
            auto wrong = rms::write_private(std::string(directory)+"/wrong.vstpreset",preset);
            rejected([&] { a.preset(wrong.path); });
            check(std::abs(a.parameters(0,1)["parameters"][0]["value"].get<double>()-.6)<1e-9,"wrong-class preset cannot replace existing program");
            unlink(file.path.c_str()); unlink(wrong.path.c_str());
        }
        for (size_t bytes : {size_t(8),size_t(4095),size_t(4096),size_t(4097),size_t(7165),size_t(8192)}) {
            auto mode = std::to_string(bytes);
            check(setenv("RMS_TEST_PLUGIN_STATE_BYTES",mode.c_str(),1) == 0,"chunked state fixture mode");
            auto chunked = s; chunked.component = {}; chunked.controller = {};
            rms::Vst3 original(chunked,48000,256);
            original.parameter(7,.37);
            auto saved = original.capture();
            check(saved.first.size() == bytes && saved.second.size() == bytes,"chunked state capture size");
            auto prefix = std::string(directory)+"/chunked-"+mode;
            chunked.component = rms::write_private(prefix+"-component",saved.first);
            auto controller = rms::write_private(prefix+"-controller",saved.second);
            for (bool separate_controller : {false,true}) {
                chunked.controller = separate_controller ? controller : rms::StateFile{};
                rms::Vst3 restored(chunked,48000,256);
                check(std::abs(restored.parameters(0,1)["parameters"][0]["value"].get<double>()-.37)<1e-9,
                    "chunked state restores controller with or without separate controller state");
                restored.note_on(60,127); restored.render(l,r,256);
                check(std::abs(l[16]-.0925f)<1e-6,"chunked state restores actual processor DSP");
                auto recaptured = restored.capture();
                check(recaptured == saved,"chunked state survives capture/restore/recapture byte for byte");
            }
            unlink(chunked.component.path.c_str()); unlink(controller.path.c_str());
        }
        check(unsetenv("RMS_TEST_PLUGIN_STATE_BYTES") == 0,"clear chunked state fixture mode");
        unlink(s.component.path.c_str()); unlink(s.controller.path.c_str()); rmdir(directory);
        std::puts("SDK ABI fixture: scan, lifecycle, nonzero render, latency, events, parameter, two states and restore passed");
        return 0;
    } catch (const std::exception& e) { std::fprintf(stderr,"vst3 test: %s\n",e.what()); return 1; }
}
