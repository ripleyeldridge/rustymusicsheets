#include "vst3.hpp"
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <stdexcept>
#include <chrono>
#include <poll.h>
#include <cerrno>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

namespace rms {
Json build_info() {
    Json revision = nullptr;
#ifdef RMS_VST3
    revision = "3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96";
#endif
    return {{"protocol",2},{"capabilities",plugin_capabilities()},{"sdk_revision",revision}};
}
namespace {
[[maybe_unused,noreturn]] void unsupported() {
    throw std::runtime_error("VST3 unavailable: rebuild helper with scripts/build-audio-host.py --vst3-sdk /ABS/PATH");
}
std::string read_regular(const std::string& path, size_t minimum, size_t maximum) {
    int fd = ::open(path.c_str(), O_RDONLY | O_CLOEXEC | O_NONBLOCK);
    if (fd < 0) throw std::runtime_error("cannot open plugin/preset file");
    struct Guard { int fd; ~Guard() { ::close(fd); } } guard{fd};
    struct stat before{}, after{};
    if (::fstat(fd, &before) || !S_ISREG(before.st_mode) || before.st_size < 0 ||
        static_cast<uint64_t>(before.st_size) < minimum || static_cast<uint64_t>(before.st_size) > maximum)
        throw std::runtime_error("plugin/preset must be a bounded regular file");
    std::string bytes(static_cast<size_t>(before.st_size), '\0');
    size_t offset = 0;
    while (offset < bytes.size()) {
        ssize_t count = ::read(fd, bytes.data()+offset, bytes.size()-offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw std::runtime_error("plugin/preset changed during read");
        offset += static_cast<size_t>(count);
    }
    char extra;
    ssize_t count;
    do { count = ::read(fd, &extra, 1); } while (count < 0 && errno == EINTR);
    if (count != 0 || ::fstat(fd, &after) || before.st_size != after.st_size ||
        before.st_mtim.tv_sec != after.st_mtim.tv_sec || before.st_mtim.tv_nsec != after.st_mtim.tv_nsec)
        throw std::runtime_error("plugin/preset changed during read; explicitly retry");
    return bytes;
}
}
std::string module_digest(const std::string& bundle) {
    absolute(bundle);
    namespace fs = std::filesystem;
    fs::path path(bundle);
    if (path.extension() != ".vst3" || !fs::is_directory(path))
        throw std::runtime_error("expected native Linux .vst3 bundle directory (not Windows VST3 or VST2 file)");
    auto payload = path / "Contents" / "x86_64-linux" / (path.stem().string() + ".so");
    std::string data = read_regular(payload.string(), 64, 512 * 1024 * 1024);
    if (std::memcmp(data.data(), "\177ELF", 4) || data[4] != 2 || data[5] != 1 ||
        static_cast<unsigned char>(data[18]) != 62 || data[19] != 0)
        throw std::runtime_error("VST3 requires native Linux x86_64 little-endian ELF; Windows/VST2/wrong architecture unsupported");
    return digest_bytes(data);
}
}

#ifdef RMS_VST3
#include "public.sdk/source/vst/hosting/module.h"
#include "public.sdk/source/vst/hosting/plugprovider.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"
#include "public.sdk/source/vst/hosting/eventlist.h"
#include "public.sdk/source/vst/vstpresetfile.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/gui/iplugview.h"
#ifdef RMS_X11
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#endif

namespace rms {
using namespace Steinberg;
using namespace Steinberg::Vst;
namespace {
constexpr size_t state_limit = 64 * 1024 * 1024;
void check(bool ok, const char* why) { if (!ok) throw std::runtime_error(why); }
bool instrument(const VST3::Hosting::ClassInfo& c) {
    return c.category() == kVstAudioEffectClass &&
        std::find(c.subCategories().begin(), c.subCategories().end(), "Instrument") != c.subCategories().end();
}
VST3::Hosting::Module::Ptr module(const std::string& path, const std::string& digest) {
    check(module_digest(path) == digest, "VST3 module SHA256 changed; explicitly rescan");
    std::string error;
    auto m = VST3::Hosting::Module::create(path, error);
    if (!m) throw std::runtime_error("VST3 module load failed: " + error.substr(0, 768));
    check(m->getFactory().classCount() <= 256, "VST3 factory exceeds class bound");
    return m;
}
// Bounded opaque stream: only used with processing quiescent.
struct Stream final : U::ImplementsNonDestroyable<U::Directly<IBStream>> {
    std::string data;
    int64 cursor = 0;
    bool overflow = false;
    explicit Stream(std::string d = {}) : data(std::move(d)) {}
    tresult PLUGIN_API read(void* p, int32 n, int32* done) override {
        if (done) *done = 0;
        if (!p || n < 0) return kInvalidArgument;
        size_t count = std::min<size_t>(n, data.size() - static_cast<size_t>(cursor));
        std::memcpy(p, data.data() + cursor, count);
        cursor += static_cast<int64>(count);
        if (done) *done = static_cast<int32>(count);
        // Like SDK MemoryStream, short reads and EOF succeed with the actual
        // byte count. JUCE discards bytes returned with a failure status.
        return kResultOk;
    }
    tresult PLUGIN_API write(void* p, int32 n, int32* done) override {
        if (done) *done = 0;
        if (!p || n < 0 || static_cast<uint64_t>(cursor) + n > state_limit) { overflow = true; return kResultFalse; }
        data.resize(std::max(data.size(), static_cast<size_t>(cursor + n)));
        std::memcpy(data.data() + cursor, p, static_cast<size_t>(n));
        cursor += n;
        if (done) *done = n;
        return kResultOk;
    }
    tresult PLUGIN_API seek(int64 n, int32 mode, int64* result) override {
        int64 base = mode == kIBSeekSet ? 0 : mode == kIBSeekCur ? cursor :
            mode == kIBSeekEnd ? static_cast<int64>(data.size()) : -1;
        if (base < 0 || n < -base || n > static_cast<int64>(data.size()) - base) return kInvalidArgument;
        cursor = base + n;
        if (result) *result = cursor;
        return kResultOk;
    }
    tresult PLUGIN_API tell(int64* p) override { if (!p) return kInvalidArgument; *p = cursor; return kResultOk; }
};
// Unlike SDK ParameterChanges these never grow vectors in process(), even when
// a plugin invents parameter IDs or emits excessive output points.
struct Queue final : U::ImplementsNonDestroyable<U::Directly<IParamValueQueue>> {
    ParamID id = 0; int32 count = 0;
    struct Point { int32 offset; double value; };
    std::array<Point, 32> points{};
    ParamID PLUGIN_API getParameterId() override { return id; }
    int32 PLUGIN_API getPointCount() override { return count; }
    tresult PLUGIN_API getPoint(int32 i, int32& offset, ParamValue& value) override {
        if (i < 0 || i >= count) return kInvalidArgument;
        offset = points[i].offset; value = points[i].value; return kResultOk;
    }
    tresult PLUGIN_API addPoint(int32 offset, ParamValue value, int32& index) override {
        if (count == static_cast<int32>(points.size()) || offset < 0 || !std::isfinite(value) || value < 0 || value > 1)
            return kResultFalse;
        index = count; points[count++] = {offset, value}; return kResultOk;
    }
};
struct Changes final : U::ImplementsNonDestroyable<U::Directly<IParameterChanges>> {
    std::array<Queue, 128> queues;
    int32 count = 0;
    bool overflow = false;
    void clear() noexcept { for (auto& q : queues) q.count = 0; count = 0; overflow = false; }
    int32 PLUGIN_API getParameterCount() override { return count; }
    IParamValueQueue* PLUGIN_API getParameterData(int32 i) override {
        return i >= 0 && i < count ? &queues[i] : nullptr;
    }
    IParamValueQueue* PLUGIN_API addParameterData(const ParamID& id, int32& index) override {
        for (int32 i = 0; i < count; ++i) if (queues[i].id == id) { index = i; return &queues[i]; }
        if (count == static_cast<int32>(queues.size())) { overflow = true; return nullptr; }
        index = count; queues[count].id = id; return &queues[count++];
    }
};
std::string utf8(const TChar* p, size_t maximum) {
    std::string s;
    for (size_t i = 0; i < 128 && p[i]; ++i) {
        uint32_t c = p[i];
        if (c >= 0xd800 && c <= 0xdbff && i + 1 < 128 && p[i+1] >= 0xdc00 && p[i+1] <= 0xdfff)
            { c = 0x10000 + ((c - 0xd800) << 10) + (p[++i] - 0xdc00); }
        else if (c >= 0xd800 && c <= 0xdfff) c = 0xfffd;
        std::string next;
        if (c < 0x80) next += static_cast<char>(c);
        else if (c < 0x800) { next += static_cast<char>(0xc0 | c >> 6); next += static_cast<char>(0x80 | (c & 63)); }
        else if (c < 0x10000) { next += static_cast<char>(0xe0 | c >> 12); next += static_cast<char>(0x80 | ((c >> 6) & 63)); next += static_cast<char>(0x80 | (c & 63)); }
        else { next += static_cast<char>(0xf0 | c >> 18); next += static_cast<char>(0x80 | ((c >> 12) & 63)); next += static_cast<char>(0x80 | ((c >> 6) & 63)); next += static_cast<char>(0x80 | (c & 63)); }
        if (s.size() + next.size() > maximum) break;
        s += next;
    }
    return s;
}
}
// Lifetime is owned by Vst3::unique_ptr, never by a plugin's callback release().
struct Vst3::Impl final : U::ImplementsNonDestroyable<U::Directly<IComponentHandler,IComponentHandler2,IPlugFrame,Linux::IRunLoop>> {
    struct Context : HostApplication {
        Impl& owner;
        explicit Context(Impl& p) : owner(p) {}
        uint32 PLUGIN_API addRef() override { return 1000; }
        uint32 PLUGIN_API release() override { return 1000; }
        tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
            if (FUnknownPrivate::iidEqual(iid,Linux::IRunLoop::iid)) return owner.queryInterface(iid,obj);
            return HostApplication::queryInterface(iid,obj);
        }
    } host{*this};
    VST3::Hosting::Module::Ptr module_;
    std::unique_ptr<PlugProvider> provider;
    IPtr<IComponent> component;
    IPtr<IEditController> controller;
    IPtr<IAudioProcessor> processor;
    EventList events{512}, output_events{512};
    Changes input, output;
    ProcessContext context{};
    std::array<int32,128> note_ids{};
    int32 next_note = 0;
    uint32_t rate, capacity, latency_ = 0;
    int32 channels = 2;
    bool active = false, processing = false;
    std::atomic<bool> failed_{false}, dirty_{false};
    std::atomic<int32> restart_{0};
    std::string class_id;
    struct Fd { IPtr<Linux::IEventHandler> handler; int fd = -1; };
    struct Timer { IPtr<Linux::ITimerHandler> handler; uint64_t interval = 0, next = 0; };
    std::array<Fd,128> fds;
    std::array<Timer,128> timers;
    IPtr<IPlugView> view;
#ifdef RMS_X11
    Display* display = nullptr;
    Window window = 0;
    Atom close_atom = 0;
#endif
    Impl(uint32_t r, uint32_t c) : rate(r), capacity(c) {
        note_ids.fill(-1);
        context.sampleRate = r;
    }
    ~Impl() override {
        close_editor();
        if (controller) controller->setComponentHandler(nullptr);
        if (processing && processor) processor->setProcessing(false);
        processing = false;
        if (active && component) component->setActive(false);
        active = false;
        processor.reset(); controller.reset(); component.reset();
        provider.reset();
        if (module_) module_->getFactory().setHostContext(nullptr);
        if (PluginContextFactory::instance().getPluginContext() == &host)
            PluginContextFactory::instance().setPluginContext(nullptr);
    }
    static uint64_t milliseconds() {
        return static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now().time_since_epoch()).count());
    }
    tresult PLUGIN_API registerEventHandler(Linux::IEventHandler* handler, int fd) override {
        if (!handler || fd < 0) return kInvalidArgument;
        for (auto& f : fds) if (!f.handler) { f = {IPtr<Linux::IEventHandler>(handler),fd}; return kResultOk; }
        return kResultFalse;
    }
    tresult PLUGIN_API unregisterEventHandler(Linux::IEventHandler* handler) override {
        bool found = false;
        for (auto& f : fds) if (f.handler.get() == handler) { f = {}; found = true; }
        return found ? kResultOk : kResultFalse;
    }
    tresult PLUGIN_API registerTimer(Linux::ITimerHandler* handler, uint64_t interval) override {
        if (!handler || !interval || interval > 3600000) return kInvalidArgument;
        for (auto& t : timers) if (!t.handler) {
            t = {IPtr<Linux::ITimerHandler>(handler),interval,milliseconds()+interval}; return kResultOk;
        }
        return kResultFalse;
    }
    tresult PLUGIN_API unregisterTimer(Linux::ITimerHandler* handler) override {
        bool found = false;
        for (auto& t : timers) if (t.handler.get() == handler) { t = {}; found = true; }
        return found ? kResultOk : kResultFalse;
    }
    tresult PLUGIN_API resizeView(IPlugView* v, ViewRect* rect) override {
#ifdef RMS_X11
        if (!display || !window || !v || !rect || rect->getWidth() < 32 || rect->getWidth() > 8192 ||
            rect->getHeight() < 32 || rect->getHeight() > 8192) return kInvalidArgument;
        XResizeWindow(display,window,static_cast<unsigned>(rect->getWidth()),static_cast<unsigned>(rect->getHeight()));
        return v->onSize(rect);
#else
        (void)v; (void)rect; return kNotImplemented;
#endif
    }
    void close_editor() {
        if (view) { view->removed(); view->setFrame(nullptr); view.reset(); }
#ifdef RMS_X11
        if (display) { if (window) XDestroyWindow(display,window); XCloseDisplay(display); }
        display = nullptr; window = 0;
#endif
    }
    void open_editor(const std::string& label) {
#ifdef RMS_X11
        if (view) { XStoreName(display,window,label.c_str()); XRaiseWindow(display,window); return; }
        check(bool(controller), "instrument has no controller/editor");
        auto candidate = owned(controller->createView(ViewType::kEditor));
        check(bool(candidate) && candidate->isPlatformTypeSupported(kPlatformTypeX11EmbedWindowID) == kResultOk,
            "plugin has no X11 editor; use generic parameters/preset");
        display = XOpenDisplay(nullptr);
        check(display, "X11 display unavailable (native Wayland embedding unsupported)");
        try {
            ViewRect rect;
            check(candidate->getSize(&rect) == kResultOk && rect.getWidth() >= 32 && rect.getWidth() <= 8192 &&
                rect.getHeight() >= 32 && rect.getHeight() <= 8192, "invalid plugin editor dimensions");
            window = XCreateSimpleWindow(display,DefaultRootWindow(display),0,0,
                static_cast<unsigned>(rect.getWidth()),static_cast<unsigned>(rect.getHeight()),0,0,0x202020);
            check(window,"cannot create editor window");
            XStoreName(display,window,label.c_str());
            XSelectInput(display,window,StructureNotifyMask|FocusChangeMask|KeyPressMask|KeyReleaseMask);
            close_atom = XInternAtom(display,"WM_DELETE_WINDOW",False); XSetWMProtocols(display,window,&close_atom,1);
            if (candidate->canResize() != kResultOk) {
                XSizeHints hints{}; hints.flags = PMinSize|PMaxSize;
                hints.min_width = hints.max_width = rect.getWidth();
                hints.min_height = hints.max_height = rect.getHeight();
                XSetWMNormalHints(display,window,&hints);
            }
            check(candidate->setFrame(this) == kResultOk,"plugin rejected editor frame");
            check(candidate->attached(reinterpret_cast<void*>(window),kPlatformTypeX11EmbedWindowID) == kResultOk,"cannot attach plugin editor");
            view = candidate;
            XMapRaised(display,window); XFlush(display);
        } catch (...) { candidate->setFrame(nullptr); close_editor(); throw; }
#else
        (void)label;
        throw std::runtime_error("X11 editor support not compiled; use generic parameters/preset");
#endif
    }
    void pump() {
        // Snapshot retained handlers: callbacks may unregister themselves.
        auto descriptors = fds;
        for (auto& f : descriptors) if (f.handler) {
            pollfd p{f.fd,POLLIN,0};
            if (::poll(&p,1,0) > 0 && (p.revents & POLLIN)) f.handler->onFDIsSet(f.fd);
        }
        uint64_t now = milliseconds();
        auto snapshot = timers;
        for (size_t i = 0; i < timers.size(); ++i) if (snapshot[i].handler && now >= snapshot[i].next) {
            timers[i].next = now + snapshot[i].interval;
            snapshot[i].handler->onTimer();
        }
#ifdef RMS_X11
        size_t events = 0;
        while (display && view && XPending(display) && events++ < 256) {
            XEvent e; XNextEvent(display,&e);
            if (e.type == ClientMessage && static_cast<Atom>(e.xclient.data.l[0]) == close_atom) { close_editor(); break; }
            if (e.type == ConfigureNotify && view->canResize() == kResultOk) {
                ViewRect rect(0,0,e.xconfigure.width,e.xconfigure.height);
                if (view->checkSizeConstraint(&rect) == kResultOk) view->onSize(&rect);
            }
            if (e.type == FocusIn || e.type == FocusOut) view->onFocus(e.type == FocusIn);
            if (e.type == KeyPress || e.type == KeyRelease) {
                char bytes[16]{}; KeySym sym{};
                int n = XLookupString(&e.xkey,bytes,sizeof(bytes),&sym,nullptr);
                char16 key = n == 1 ? static_cast<unsigned char>(bytes[0]) : 0;
                if (e.type == KeyPress) view->onKeyDown(key,0,0);
                else view->onKeyUp(key,0,0);
            }
        }
#endif
    }
    tresult PLUGIN_API beginEdit(ParamID) override { return kResultOk; }
    tresult PLUGIN_API setDirty(TBool state) override {
        if (state) dirty_ = true; // Only a successful host checkpoint clears dirty.
        return kResultOk;
    }
    tresult PLUGIN_API requestOpenEditor(FIDString) override { return kNotImplemented; }
    tresult PLUGIN_API startGroupEdit() override { return kResultOk; }
    tresult PLUGIN_API finishGroupEdit() override { return kResultOk; }
    tresult PLUGIN_API performEdit(ParamID id, ParamValue value) override {
        // Editor operations run on the main thread; Live quiesces around pump.
        if (!std::isfinite(value) || value < 0 || value > 1) return kInvalidArgument;
        int32 i = 0;
        auto* q = input.addParameterData(id, i);
        if (!q || q->addPoint(0, value, i) != kResultOk) { failed_ = true; return kResultFalse; }
        dirty_ = true; return kResultOk;
    }
    tresult PLUGIN_API endEdit(ParamID) override { return kResultOk; }
    tresult PLUGIN_API restartComponent(int32 flags) override { restart_.fetch_or(flags); return kResultOk; }
    void event(uint8_t pitch, uint8_t velocity, bool on) noexcept {
        Event e{};
        e.type = on ? Event::kNoteOnEvent : Event::kNoteOffEvent;
        if (on) {
            if (next_note == INT32_MAX) next_note = 0;
            note_ids[pitch] = next_note++;
            e.noteOn.channel = 0; e.noteOn.pitch = pitch; e.noteOn.velocity = velocity / 127.f;
            e.noteOn.noteId = note_ids[pitch]; e.noteOn.tuning = 0;
        } else {
            e.noteOff.channel = 0; e.noteOff.pitch = pitch; e.noteOff.velocity = 0;
            e.noteOff.noteId = note_ids[pitch]; note_ids[pitch] = -1;
        }
        if (events.addEvent(e) != kResultOk) failed_ = true;
    }
    void restore(const std::string& a, const std::string& b) {
        if (!a.empty()) {
            Stream s(a);
            check(component->setState(&s) == kResultOk, "plugin rejected component state");
            if (controller) { s.cursor = 0; check(controller->setComponentState(&s) == kResultOk, "controller rejected component state"); }
        }
        if (!b.empty()) { check(bool(controller), "state requires controller"); Stream s(b); check(controller->setState(&s) == kResultOk, "controller rejected state"); }
    }
};
Json plugin_capabilities() {
    Json caps = Json::array({"soundfont", "jack", "vst3", "plugin_state", "parameters", "presets"});
#ifdef RMS_X11
    if (auto* display = XOpenDisplay(nullptr)) { caps.push_back("editor_x11"); XCloseDisplay(display); }
#endif
    return caps;
}
Json scan_plugin(const Json& j) {
    auto path = text(j, "path", 4096), hash = text(j, "module_sha256", 64);
    hex64(hash);
    auto m = module(path, hash);
    Json classes = Json::array();
    for (const auto& c : m->getFactory().classInfos()) if (instrument(c)) {
        check(classes.size() < 128, "too many instrument classes");
        check(c.name().size() <= 256 && c.vendor().size() <= 256 && c.version().size() <= 128, "oversized class labels");
        classes.push_back({{"class_id", c.ID().toString()}, {"name", c.name()}, {"vendor", c.vendor()}, {"version", c.version()}});
    }
    check(!classes.empty(), "module has no supported VST3 instrument classes");
    return {{"path", path}, {"module_sha256", hash}, {"classes", classes}};
}
Vst3::Vst3(const Source& s, uint32_t rate, uint32_t capacity) : impl_(std::make_unique<Impl>(rate, capacity)) {
    auto& p = *impl_;
    p.class_id = s.class_id;
    p.module_ = module(s.path, s.module_sha256);
    PlugProvider::setErrorStream(nullptr);
    PluginContextFactory::instance().setPluginContext(&p.host);
    p.module_->getFactory().setHostContext(&p.host);
    bool found = false;
    for (const auto& c : p.module_->getFactory().classInfos()) if (c.ID().toString() == s.class_id && instrument(c)) {
        p.provider = std::make_unique<PlugProvider>(p.module_->getFactory(), c, true);
        check(p.provider->initialize(), "VST3 component/controller initialization failed");
        found = true; break;
    }
    check(found, "selected instrument class no longer exists");
    p.component = p.provider->getComponentPtr();
    p.controller = p.provider->getControllerPtr();
    p.processor = U::cast<IAudioProcessor>(p.component);
    check(bool(p.processor), "class has no IAudioProcessor");
    check(p.processor->canProcessSampleSize(kSample32) == kResultOk, "instrument does not support float32");
    check(p.component->getBusCount(kAudio, kInput) == 0, "effects/audio-input instruments unsupported");
    check(p.component->getBusCount(kAudio, kOutput) == 1, "requires exactly one main audio output bus");
    BusInfo bus{};
    check(p.component->getBusInfo(kAudio, kOutput, 0, bus) == kResultOk && bus.busType == kMain &&
        (bus.channelCount == 1 || bus.channelCount == 2), "requires mono/stereo main output");
    p.channels = bus.channelCount;
    SpeakerArrangement arrangement = p.channels == 1 ? SpeakerArr::kMono : SpeakerArr::kStereo;
    check(p.processor->setBusArrangements(nullptr, 0, &arrangement, 1) == kResultOk, "output bus arrangement rejected");
    check(p.component->getBusCount(kEvent, kInput) >= 1, "instrument requires note event input");
    for (int32 i = 0; i < p.component->getBusCount(kEvent, kInput); ++i)
        check(p.component->activateBus(kEvent, kInput, i, i == 0) == kResultOk, "cannot activate event bus");
    check(p.component->activateBus(kAudio, kOutput, 0, true) == kResultOk, "cannot activate output");
    if (p.controller) {
        check(p.controller->getParameterCount() >= 0 && p.controller->getParameterCount() <= 4096, "parameter count exceeds 4096");
        check(p.controller->setComponentHandler(&p) == kResultOk, "controller rejected host handler");
    }
    std::string a, b;
    if (!s.component.path.empty()) a = read_private(s.component);
    if (!s.controller.path.empty()) b = read_private(s.controller);
    p.restore(a, b);
    ProcessSetup setup{kRealtime, kSample32, static_cast<int32>(capacity), static_cast<double>(rate)};
    check(p.processor->setupProcessing(setup) == kResultOk, "VST3 setupProcessing failed");
    p.latency_ = p.processor->getLatencySamples();
    check(p.latency_ <= uint64_t(rate) * 10, "plugin latency exceeds ten seconds");
    check(p.component->setActive(true) == kResultOk, "VST3 activation failed"); p.active = true;
    auto started = p.processor->setProcessing(true);
    check(started == kResultOk || started == kNotImplemented, "VST3 processing activation failed"); p.processing = true;
    p.latency_ = p.processor->getLatencySamples();
    check(p.latency_ <= uint64_t(rate) * 10, "plugin latency exceeds ten seconds after activation");
    p.restart_.fetch_and(~kLatencyChanged);
}
Vst3::~Vst3() = default;
void Vst3::note_on(uint8_t p, uint8_t v) noexcept { impl_->event(p,v,true); }
void Vst3::note_off(uint8_t p) noexcept { impl_->event(p,0,false); }
void Vst3::silence() noexcept {
    for (uint32_t i = 0; i < 128; ++i) if (impl_->note_ids[i] >= 0) impl_->event(static_cast<uint8_t>(i),0,false);
}
void Vst3::render(float* l, float* r, uint32_t frames) noexcept {
    auto& p = *impl_;
    std::fill_n(l, frames, 0); std::fill_n(r, frames, 0);
    if (frames > p.capacity || p.failed_) { p.failed_ = true; return; }
    float* buffers[2]{l,r};
    AudioBusBuffers bus{}; bus.numChannels = p.channels; bus.channelBuffers32 = buffers;
    ProcessData data{};
    data.processMode = kRealtime; data.symbolicSampleSize = kSample32; data.numSamples = static_cast<int32>(frames);
    data.numOutputs = 1; data.outputs = &bus;
    data.inputEvents = &p.events; data.outputEvents = &p.output_events;
    data.inputParameterChanges = &p.input; data.outputParameterChanges = &p.output; data.processContext = &p.context;
    if (p.processor->process(data) != kResultOk || p.output.overflow) p.failed_ = true;
    if (p.channels == 1) std::copy_n(l, frames, r);
    p.events.clear(); p.output_events.clear(); p.input.clear(); p.output.clear();
    p.context.projectTimeSamples += frames;
    p.context.continousTimeSamples += frames;
}
bool Vst3::failed() const noexcept { return impl_->failed_; }
uint32_t Vst3::latency() const noexcept { return impl_->latency_; }
void Vst3::transport(double seconds, double bpm, bool playing, double quarters, uint64_t continuous) noexcept {
    auto& c = impl_->context;
    c.state = ProcessContext::kTempoValid | ProcessContext::kContTimeValid |
        (playing ? ProcessContext::kPlaying : 0);
    c.sampleRate = impl_->rate; c.projectTimeSamples = static_cast<int64>(seconds * impl_->rate);
    c.projectTimeMusic = quarters;
    c.continousTimeSamples = static_cast<int64>(continuous);
    c.tempo = bpm;
}
Json Vst3::parameters(uint32_t offset, uint32_t limit) {
    auto& p = *impl_; check(bool(p.controller), "plugin has no edit controller");
    auto total = p.controller->getParameterCount();
    check(total >= 0 && total <= 4096, "parameter count exceeds bound");
    check(offset <= static_cast<uint32_t>(total) && limit >= 1 && limit <= 128, "invalid parameter page");
    Json list = Json::array();
    for (uint32_t i = offset; i < static_cast<uint32_t>(total) && i < offset + limit; ++i) {
        ParameterInfo info{}; check(p.controller->getParameterInfo(static_cast<int32>(i), info) == kResultOk, "cannot inspect parameter");
        double value = p.controller->getParamNormalized(info.id);
        check(std::isfinite(value) && value >= 0 && value <= 1 && std::isfinite(info.defaultNormalizedValue) &&
            info.defaultNormalizedValue >= 0 && info.defaultNormalizedValue <= 1 && info.stepCount >= 0, "invalid plugin parameter");
        list.push_back({{"id", info.id}, {"title", utf8(info.title,256)}, {"units", utf8(info.units,128)},
            {"value", value}, {"default_value", info.defaultNormalizedValue}, {"step_count", info.stepCount}, {"flags", static_cast<uint32_t>(info.flags)}});
    }
    return {{"total", total}, {"offset", offset}, {"parameters", list}};
}
void Vst3::parameter(uint32_t id, double value) {
    auto& p = *impl_; check(bool(p.controller), "plugin has no controller");
    bool found = false;
    for (int32 i = 0; i < p.controller->getParameterCount(); ++i) {
        ParameterInfo info{};
        if (p.controller->getParameterInfo(i,info) == kResultOk && info.id == id) {
            check(!(info.flags & ParameterInfo::kIsReadOnly), "parameter is read-only"); found = true; break;
        }
    }
    check(found && std::isfinite(value) && value >= 0 && value <= 1, "unknown parameter or invalid value");
    check(p.controller->setParamNormalized(id,value) == kResultOk && p.performEdit(id,value) == kResultOk, "parameter update failed");
}
std::pair<std::string,std::string> Vst3::capture() {
    auto& p = *impl_;
    // Controller edits queued while paused must reach the processor before
    // component state is captured. SDK process(0) flushes events/parameters
    // without advancing the score or exposing audio.
    if (p.input.count || p.events.getEventCount()) {
        float l[1]{}, r[1]{};
        render(l,r,0);
        check(!p.failed_, "plugin could not flush paused parameter changes");
    }
    Stream a,b;
    check(p.component->getState(&a) == kResultOk && !a.overflow, "component state capture failed");
    if (p.controller && p.controller->getState(&b) != kResultOk) b.data.clear();
    check(!b.overflow && a.data.size() + b.data.size() <= state_limit, "combined state exceeds 64MiB");
    return {std::move(a.data),std::move(b.data)};
}
void Vst3::preset(const std::string& path) {
    absolute(path);
    std::string bytes = read_regular(path,48,state_limit);
    Stream stream(std::move(bytes));
    FUID uid; check(uid.fromString(impl_->class_id.c_str()), "invalid class identity");
    PresetFile preset(&stream);
    check(preset.readChunkList() && preset.getClassID() == uid, "preset class does not match selected instrument");
    check(preset.restoreComponentState(impl_->component), "preset component restore failed");
    if (impl_->controller) {
        check(preset.restoreComponentState(impl_->controller), "preset controller component restore failed");
        if (preset.contains(kControllerState)) check(preset.restoreControllerState(impl_->controller), "preset controller restore failed");
    }
    impl_->dirty_ = true;
}
bool Vst3::dirty() const noexcept { return impl_->dirty_; }
void Vst3::clean() noexcept { impl_->dirty_ = false; }
void Vst3::editor(bool open, const std::string& label) { if (open) impl_->open_editor(label); else impl_->close_editor(); }
uint64_t Vst3::editor_window() const noexcept {
#ifdef RMS_X11
    return impl_->window;
#else
    return 0;
#endif
}
bool Vst3::pump() {
    impl_->pump();
    auto& p = *impl_;
    int32 flags = p.restart_.exchange(0);
    if (flags & kReloadComponent)
        throw std::runtime_error("plugin requested full component reload; save where possible and explicitly reload instrument");
    if (!(flags & (kLatencyChanged | kIoChanged))) return false;
    try {
        if (p.input.count || p.events.getEventCount()) {
            float l[1]{}, r[1]{}; render(l,r,0);
            check(!p.failed_, "plugin could not flush before reconfiguration");
        }
        if (p.processing) { p.processor->setProcessing(false); p.processing = false; }
        if (p.active) { check(p.component->setActive(false) == kResultOk,"plugin deactivation failed"); p.active = false; }
        if (flags & kIoChanged) {
            check(p.component->getBusCount(kAudio,kInput) == 0 && p.component->getBusCount(kAudio,kOutput) == 1,
                "reconfigured plugin requires unsupported audio buses");
            BusInfo bus{};
            check(p.component->getBusInfo(kAudio,kOutput,0,bus) == kResultOk && bus.busType == kMain &&
                (bus.channelCount == 1 || bus.channelCount == 2),"reconfigured output must remain mono/stereo");
            p.channels = bus.channelCount;
            SpeakerArrangement arrangement = p.channels == 1 ? SpeakerArr::kMono : SpeakerArr::kStereo;
            check(p.processor->setBusArrangements(nullptr,0,&arrangement,1) == kResultOk,"reconfigured arrangement rejected");
            check(p.component->activateBus(kAudio,kOutput,0,true) == kResultOk,"reconfigured bus activation failed");
        }
        ProcessSetup setup{kRealtime,kSample32,static_cast<int32>(p.capacity),static_cast<double>(p.rate)};
        check(p.processor->setupProcessing(setup) == kResultOk,"plugin reprepare failed");
        check(p.component->setActive(true) == kResultOk,"plugin reactivation failed"); p.active = true;
        auto result = p.processor->setProcessing(true);
        check(result == kResultOk || result == kNotImplemented,"plugin processing restart failed"); p.processing = true;
        p.latency_ = p.processor->getLatencySamples();
        check(p.latency_ <= uint64_t(p.rate)*10,"reconfigured latency exceeds ten seconds");
        p.restart_.fetch_and(~(kLatencyChanged|kIoChanged));
        return true;
    } catch (...) { p.failed_ = true; throw; }
}
}
#else
namespace rms {
struct Vst3::Impl {};
Json plugin_capabilities() { return Json::array({"soundfont", "jack"}); }
Json scan_plugin(const Json&) { unsupported(); }
Vst3::Vst3(const Source&, uint32_t, uint32_t) { unsupported(); }
Vst3::~Vst3() = default;
void Vst3::note_on(uint8_t,uint8_t) noexcept {}
void Vst3::note_off(uint8_t) noexcept {}
void Vst3::silence() noexcept {}
void Vst3::render(float*,float*,uint32_t) noexcept {}
bool Vst3::failed() const noexcept { return true; }
uint32_t Vst3::latency() const noexcept { return 0; }
void Vst3::transport(double,double,bool,double,uint64_t) noexcept {}
Json Vst3::parameters(uint32_t,uint32_t) { unsupported(); }
void Vst3::parameter(uint32_t,double) { unsupported(); }
std::pair<std::string,std::string> Vst3::capture() { unsupported(); }
void Vst3::preset(const std::string&) { unsupported(); }
bool Vst3::dirty() const noexcept { return false; }
void Vst3::clean() noexcept {}
void Vst3::editor(bool,const std::string&) { unsupported(); }
uint64_t Vst3::editor_window() const noexcept { return 0; }
bool Vst3::pump() { return false; }
}
#endif
