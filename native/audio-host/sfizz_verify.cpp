// Opt-in real-plugin verification. No JACK client, audio device or user samples.
#include "vst3.hpp"
#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <stdexcept>
#include <thread>

namespace {
void require(bool ok, const char* message) {
    if (!ok) throw std::runtime_error(message);
}
struct AudioCheck {
    double peak = 0;
    std::array<double,3> amplitudes{};
};
AudioCheck chord(rms::Vst3& plugin) {
    std::array<float,4097> left{},right{};
    const std::array<double,3> frequencies{220,220*std::pow(2.,4./12),220*std::pow(2.,7./12)};
    std::array<double,3> real{},imaginary{};
    size_t measured = 0;
    plugin.transport(0,120,true);
    for (uint8_t pitch : {60,64,67}) plugin.note_on(pitch,90);
    AudioCheck result;
    for (int block=0;block<12;++block) {
        plugin.render(left.data(),right.data(),left.size());
        require(!plugin.failed(),"plugin render failed");
        for (size_t i=0;i<left.size();++i) {
            require(std::isfinite(left[i]) && std::isfinite(right[i]),"nonfinite plugin output");
            result.peak = std::max({result.peak,std::abs(double(left[i])),std::abs(double(right[i]))});
            if (block>=2) {
                const double seconds=(block*left.size()+i)/48000.;
                for (size_t voice=0;voice<frequencies.size();++voice) {
                    const double phase=2*std::acos(-1.)*frequencies[voice]*seconds;
                    real[voice] += left[i]*std::cos(phase);
                    imaginary[voice] += left[i]*std::sin(phase);
                }
                ++measured;
            }
        }
    }
    plugin.silence();
    plugin.render(left.data(),right.data(),left.size());
    require(!plugin.failed() && result.peak>1e-5,"simultaneous generated notes must produce audio");
    for (size_t voice=0;voice<frequencies.size();++voice) {
        result.amplitudes[voice]=2*std::hypot(real[voice],imaginary[voice])/measured;
        require(result.amplitudes[voice]>.005,"all three sample-based chord voices must be audible in the rendered buffer");
    }
    return result;
}
}
int main(int argc,char** argv) {
    try {
        require(argc==4 || argc==5,"usage: rms-sfizz-verify BUNDLE PRESET FRESH_PRIVATE_STATE_DIRECTORY [EDITOR_SECONDS]");
        rms::Source source;
        source.vst3 = true; source.path = argv[1];
        source.module_sha256 = rms::module_digest(source.path);
        auto scan = rms::scan_plugin({{"path",source.path},{"module_sha256",source.module_sha256}});
        for (const auto& entry : scan["classes"])
            if (entry["name"]=="sfizz" && entry["vendor"]=="SFZTools" && entry["version"]=="1.2.3")
                source.class_id = entry["class_id"];
        require(source.class_id=="E8FAB71815ED46E38B5983101E12993F","official sfizz 1.2.3 stereo class required, not sfizz-multi");
        AudioCheck initial;
        std::pair<std::string,std::string> saved;
        rms::Json parameters;
        {
            rms::Vst3 plugin(source,48000,65536);
            parameters = plugin.parameters(0,128);
            require(parameters["total"].get<uint32_t>()>0,"generic parameters missing");
            // Exercise all generic parameter pages, including a writable edit.
            uint32_t total = parameters["total"];
            for (uint32_t offset=0;offset<total;offset+=128) plugin.parameters(offset,128);
            bool edited = false;
            for (const auto& parameter : parameters["parameters"]) if (!(parameter["flags"].get<uint32_t>()&2)) {
                plugin.parameter(parameter["id"],parameter["value"]);
                edited = true; break;
            }
            require(edited,"no writable generic parameter");
            plugin.preset(argv[2]);
            if (argc==5) {
                int seconds = std::stoi(argv[4]);
                require(seconds>=1 && seconds<=120,"editor duration must be 1..120 seconds");
                plugin.editor(true,"LinuxSheets sfizz isolated verification");
                require(plugin.editor_window()!=0,"X11 editor not opened");
                std::printf("editor_opened\n"); std::fflush(stdout);
                auto end=std::chrono::steady_clock::now()+std::chrono::seconds(seconds);
                while (std::chrono::steady_clock::now()<end) {
                    plugin.pump(); std::this_thread::sleep_for(std::chrono::milliseconds(10));
                }
                plugin.editor(false);
                require(plugin.editor_window()==0,"X11 editor not closed");
                plugin.editor(true); plugin.pump(); plugin.editor(false);
            }
            initial = chord(plugin);
            saved = plugin.capture();
            require(!saved.first.empty(),"component checkpoint empty");
            require(saved.first.find("generated.sfz")!=std::string::npos,"component checkpoint must retain the generated SFZ path");
            source.component = rms::write_private(std::string(argv[3])+"/component.bin",saved.first);
            if (!saved.second.empty())
                source.controller = rms::write_private(std::string(argv[3])+"/controller.bin",saved.second);
        }
        rms::Vst3 restored(source,48000,65536);
        auto restored_audio = chord(restored);
        require(restored.capture()==saved,"opaque checkpoint changed after restore");
        rms::Json result = {{"scan",scan},{"parameters",parameters["total"]},
            {"initial_peak",initial.peak},{"restored_peak",restored_audio.peak},
            {"initial_voice_amplitudes",initial.amplitudes},{"restored_voice_amplitudes",restored_audio.amplitudes},
            {"component_bytes",saved.first.size()},{"controller_bytes",saved.second.size()},
            {"checkpoint_sha256",rms::digest_bytes(saved.first)},
            {"sample_rate",48000},{"simultaneous_pitches",{60,64,67}},
            {"opaque_roundtrip",true},{"editor",argc==5}};
        std::puts(result.dump(2).c_str());
        return 0;
    } catch (const std::exception& error) {
        std::fprintf(stderr,"sfizz verification: %s\n",error.what()); return 1;
    }
}
