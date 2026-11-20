// Original LinuxSheets test fixture; distributed under the repository license.
// No commercial/user plugin code, resources or SDK example implementation.
#include "public.sdk/source/vst/vstaudioeffect.h"
#include "public.sdk/source/vst/vsteditcontroller.h"
#include "public.sdk/source/main/pluginfactory.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/base/funknownimpl.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include <array>
#include <chrono>
#include <cmath>
#include <csignal>
#include <cstdlib>
#include <cstring>
#include <string>
#include <thread>
#include <unistd.h>
#ifdef RMS_X11
#include <X11/Xlib.h>
#endif

using namespace Steinberg;
using namespace Steinberg::Vst;
namespace {
const FUID processor_id(0x6C736874,0x65737431,0xA1023456,0x789ABCDE);
const FUID controller_id(0x6C736874,0x65737432,0xA1023456,0x789ABCDE);
constexpr int delay_frames = 16;
size_t state_bytes() {
    // Explicit test-only serialization mode; default fixtures stay one double.
    const char* mode = std::getenv("RMS_TEST_PLUGIN_STATE_BYTES");
    if (!mode) return sizeof(double);
    char* end = nullptr;
    auto size = std::strtoul(mode,&end,10);
    return end && !*end && size >= sizeof(double) && size <= 16384 ? size : sizeof(double);
}
double read_value(IBStream* s, bool& ok, bool chunked = false) {
    double value = 0;
    int32 count = 0;
    if (chunked && std::getenv("RMS_TEST_PLUGIN_STATE_BYTES")) {
        // Match consumers such as JUCE: only append successful reads, even
        // when the final read returns fewer bytes than the requested block.
        std::string bytes;
        std::array<char,4096> block{};
        while (s) {
            count = 0;
            if (s->read(block.data(),block.size(),&count) != kResultOk || count <= 0) break;
            if (count > static_cast<int32>(block.size()) || bytes.size() + count > 16384) { ok = false; return 0; }
            bytes.append(block.data(),static_cast<size_t>(count));
        }
        ok = bytes.size() == state_bytes();
        if (!ok) return 0;
        std::memcpy(&value,bytes.data(),sizeof(value));
        for (size_t i = sizeof(value); i < bytes.size(); ++i)
            if (bytes[i] != 's') { ok = false; return 0; }
    } else {
        ok = s && s->read(&value,sizeof(value),&count) == kResultOk && count == sizeof(value);
    }
    ok = ok && std::isfinite(value) && value >= 0 && value <= 1;
    return value;
}
tresult write_value(IBStream* s, double value) {
    if (!s) return kInvalidArgument;
    std::string bytes(state_bytes(),'s');
    std::memcpy(bytes.data(),&value,sizeof(value));
    return s->write(bytes.data(),static_cast<int32>(bytes.size()),nullptr);
}
class Processor final : public AudioEffect {
    double level = .5;
    std::array<float,128> notes{};
    std::array<float,32> delay{};
    uint32 latency_frames = delay_frames;
    size_t cursor = 0;
    bool chunk_checks = std::getenv("RMS_TEST_PLUGIN_CHUNK_CHECKS") != nullptr;
    int64 expected_samples = 0;
    double expected_quarters = 0;
    void apply(ParamID id, double value) {
        if (id == 7) level = value;
        if (id == 8) { latency_frames = value >= .5 ? 32 : 16; delay.fill(0); cursor = 0; }
    }
public:
    Processor() { setControllerClass(controller_id); }
    static FUnknown* create(void*) { return static_cast<IAudioProcessor*>(new Processor); }
    tresult PLUGIN_API initialize(FUnknown* context) override {
        auto result = AudioEffect::initialize(context);
        if (result != kResultOk) return result;
        addEventInput(STR16("Notes"),1);
        addAudioOutput(STR16("Output"),std::getenv("RMS_TEST_PLUGIN_MONO") ? SpeakerArr::kMono : SpeakerArr::kStereo);
        return kResultOk;
    }
    uint32 PLUGIN_API getLatencySamples() override { return latency_frames; }
    tresult PLUGIN_API setState(IBStream* s) override {
        bool ok; double value = read_value(s,ok,true);
        if (!ok) return kResultFalse;
        level = value; return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream* s) override { return write_value(s,level); }
    tresult PLUGIN_API setupProcessing(ProcessSetup& setup) override {
        // A valid instrument may reject host scratch-sized processing blocks.
        if (setup.maxSamplesPerBlock > 1024) return kResultFalse;
        return AudioEffect::setupProcessing(setup);
    }
    tresult PLUGIN_API setProcessing(TBool state) override {
        (void)state; notes.fill(0); delay.fill(0); cursor = 0; return kResultOk;
    }
    tresult PLUGIN_API process(ProcessData& d) override {
        if (d.symbolicSampleSize != kSample32) return kResultFalse;
        if (d.numSamples > processSetup.maxSamplesPerBlock) return kResultFalse;
        if (chunk_checks && d.processContext) {
            const auto& c = *d.processContext;
            if (c.projectTimeSamples != expected_samples || c.continousTimeSamples != expected_samples ||
                std::abs(c.projectTimeMusic-expected_quarters)>1e-9) return kResultFalse;
            expected_samples += d.numSamples;
            expected_quarters += d.numSamples*c.tempo/(60*c.sampleRate);
        }
        if (d.processContext && (d.processContext->state & ProcessContext::kTimeSigValid))
            return kResultFalse; // Fixture plans have no recognized time signature.
        if (d.numSamples == 0 && d.inputParameterChanges) {
            for (int32 q = 0; q < d.inputParameterChanges->getParameterCount(); ++q) {
                auto* queue = d.inputParameterChanges->getParameterData(q);
                if (!queue) continue;
                for (int32 point = 0; point < queue->getPointCount(); ++point) {
                    int32 offset = 0; double value = 0;
                    if (queue->getPoint(point,offset,value) == kResultOk && offset == 0) apply(queue->getParameterId(),value);
                }
            }
        }
        int32 event_index = 0;
        for (int32 i = 0; i < d.numSamples; ++i) {
            if (d.inputParameterChanges) for (int32 q = 0; q < d.inputParameterChanges->getParameterCount(); ++q) {
                auto* queue = d.inputParameterChanges->getParameterData(q);
                if (!queue) continue;
                for (int32 p = 0; p < queue->getPointCount(); ++p) {
                    int32 offset; double value;
                    if (queue->getPoint(p,offset,value) == kResultOk && offset == i) apply(queue->getParameterId(),value);
                }
            }
            if (d.inputEvents) while (event_index < d.inputEvents->getEventCount()) {
                Event e{};
                if (d.inputEvents->getEvent(event_index,e) != kResultOk) return kResultFalse;
                if (e.sampleOffset > i) break;
                if (e.type == Event::kNoteOnEvent && e.noteOn.pitch >= 0 && e.noteOn.pitch < 128) {
                    if (chunk_checks && notes[e.noteOn.pitch] != 0) return kResultFalse;
                    notes[e.noteOn.pitch] = e.noteOn.velocity;
                }
                if (e.type == Event::kNoteOffEvent && e.noteOff.pitch >= 0 && e.noteOff.pitch < 128)
                    notes[e.noteOff.pitch] = 0;
                ++event_index;
            }
            float sum = 0; for (float note : notes) sum += note;
            float output = delay[cursor];
            delay[cursor] = static_cast<float>(sum * level * .25);
            cursor = (cursor + 1) % latency_frames;
            if (d.numOutputs) for (int32 c = 0; c < d.outputs[0].numChannels; ++c)
                d.outputs[0].channelBuffers32[c][i] = output;
        }
        return kResultOk;
    }
};
#ifdef RMS_X11
class ModalTimer final : public U::Implements<U::Directly<Linux::ITimerHandler>> {
    unsigned long milliseconds_;
    bool fired_ = false;
public:
    explicit ModalTimer(unsigned long milliseconds) : milliseconds_(milliseconds) {}
    void PLUGIN_API onTimer() override {
        if (fired_) return;
        fired_ = true;
        std::this_thread::sleep_for(std::chrono::milliseconds(milliseconds_));
    }
};
class FixtureView final : public EditorView {
    Display* display = nullptr;
    Window window = 0;
    IPtr<Linux::IRunLoop> loop_;
    IPtr<ModalTimer> modal_timer_;
    void draw() {
        if (!display || !window) return;
        XClearWindow(display,window);
        auto gc = DefaultGC(display,DefaultScreen(display));
        XSetForeground(display,gc,0xffffff);
        const char* title = "LinuxSheets disposable VST3 fixture";
        const char* keys = "Type 1..9 to change Level. R requests resize.";
        XDrawString(display,window,gc,20,30,title,static_cast<int>(std::strlen(title)));
        XDrawString(display,window,gc,20,60,keys,static_cast<int>(std::strlen(keys)));
        char value[64]; std::snprintf(value,sizeof(value),"Level %.2f",controller->getParamNormalized(7));
        XDrawString(display,window,gc,20,90,value,static_cast<int>(std::strlen(value)));
        XFlush(display);
    }
public:
    explicit FixtureView(EditController* c) : EditorView(c) { rect = ViewRect(0,0,480,160); }
    tresult PLUGIN_API isPlatformTypeSupported(FIDString type) override {
        return !std::strcmp(type,kPlatformTypeX11EmbedWindowID) ? kResultOk : kResultFalse;
    }
    tresult PLUGIN_API attached(void* parent,FIDString type) override {
        if (isPlatformTypeSupported(type) != kResultOk) return kResultFalse;
        display = XOpenDisplay(nullptr); if (!display) return kResultFalse;
        window = XCreateSimpleWindow(display,reinterpret_cast<Window>(parent),0,0,480,160,0,0,0x203050);
        XMapWindow(display,window); XSync(display,False); draw();
        if (const char* delay = std::getenv("RMS_TEST_PLUGIN_MODAL_MS")) {
            char* end = nullptr;
            auto milliseconds = std::strtoul(delay,&end,10);
            if (!end || *end || milliseconds > 10000) return kInvalidArgument;
            if (std::getenv("RMS_TEST_PLUGIN_MODAL_TIMER")) {
                loop_ = U::cast<Linux::IRunLoop>(plugFrame);
                if (!loop_) return kResultFalse;
                modal_timer_ = owned(new ModalTimer(milliseconds));
                if (loop_->registerTimer(modal_timer_,20) != kResultOk) return kResultFalse;
            } else {
                std::this_thread::sleep_for(std::chrono::milliseconds(milliseconds));
            }
        }
        return kResultOk;
    }
    tresult PLUGIN_API removed() override {
        if (loop_ && modal_timer_) loop_->unregisterTimer(modal_timer_);
        modal_timer_.reset(); loop_.reset();
        if (display) { if (window) XDestroyWindow(display,window); XCloseDisplay(display); }
        display = nullptr; window = 0; return kResultOk;
    }
    tresult PLUGIN_API onKeyDown(char16 key,int16,int16) override {
        if (key >= '1' && key <= '9') {
            double value = (key-'0')/10.;
            controller->beginEdit(7); controller->setParamNormalized(7,value);
            controller->performEdit(7,value); controller->endEdit(7); draw(); return kResultOk;
        }
        if ((key == 'r' || key == 'R') && plugFrame) {
            ViewRect size(0,0,640,240); return plugFrame->resizeView(this,&size);
        }
        return kResultFalse;
    }
    tresult PLUGIN_API canResize() override { return kResultOk; }
    tresult PLUGIN_API checkSizeConstraint(ViewRect* r) override {
        if (!r) return kInvalidArgument;
        r->right = std::max(r->left+320,r->right); r->bottom = std::max(r->top+120,r->bottom); return kResultOk;
    }
    tresult PLUGIN_API onSize(ViewRect* r) override {
        auto result = EditorView::onSize(r);
        if (display && window) XResizeWindow(display,window,static_cast<unsigned>(r->getWidth()),static_cast<unsigned>(r->getHeight()));
        draw(); return result;
    }
};
#endif
class Controller final : public EditController {
public:
    static FUnknown* create(void*) { return static_cast<IEditController*>(new Controller); }
    tresult PLUGIN_API initialize(FUnknown* context) override {
        auto result = EditController::initialize(context);
        if (result == kResultOk) {
            parameters.addParameter(STR16("Level"),STR16(""),0,.5,ParameterInfo::kCanAutomate,7);
            parameters.addParameter(STR16("Latency switch (test-only, not preset state)"),STR16(""),1,0,ParameterInfo::kCanAutomate,8);
        }
        return result;
    }
    tresult PLUGIN_API setParamNormalized(ParamID id, ParamValue value) override {
        auto result = EditController::setParamNormalized(id,value);
        if (result == kResultOk && id == 8 && componentHandler) componentHandler->restartComponent(kLatencyChanged);
        return result;
    }
    tresult PLUGIN_API setComponentState(IBStream* s) override {
        bool ok; double value = read_value(s,ok);
        return ok ? setParamNormalized(7,value) : kResultFalse;
    }
    tresult PLUGIN_API setState(IBStream* s) override {
        bool ok; double value = read_value(s,ok,true);
        return ok ? setParamNormalized(7,value) : kResultFalse;
    }
    tresult PLUGIN_API getState(IBStream* s) override {
        return write_value(s,getParamNormalized(7));
    }
#ifdef RMS_X11
    IPlugView* PLUGIN_API createView(FIDString name) override {
        return !std::strcmp(name,ViewType::kEditor) ? new FixtureView(this) : nullptr;
    }
#endif
};
}
bool InitModule() {
    // Explicit test-only crash/hang injection. Never present in production host.
    const char* mode = std::getenv("RMS_TEST_PLUGIN_FAULT");
    if (mode && !std::strcmp(mode,"crash")) std::raise(SIGSEGV);
    if (mode && !std::strcmp(mode,"hang")) for (;;) pause();
    return true;
}
bool DeinitModule() { return true; }
BEGIN_FACTORY_DEF("LinuxSheets tests","https://github.com/","")
DEF_CLASS2(INLINE_UID_FROM_FUID(processor_id),PClassInfo::kManyInstances,kVstAudioEffectClass,
    "LinuxSheets Deterministic Instrument",Vst::kDistributable,"Instrument|Synth","1.0.0",kVstVersionString,Processor::create)
DEF_CLASS2(INLINE_UID_FROM_FUID(controller_id),PClassInfo::kManyInstances,kVstComponentControllerClass,
    "LinuxSheets Test Controller",0,"","1.0.0",kVstVersionString,Controller::create)
END_FACTORY
