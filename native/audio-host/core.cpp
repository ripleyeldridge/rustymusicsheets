#include "core.hpp"

#include <algorithm>
#include <cmath>
#include <limits>
#include <stdexcept>
#include <unordered_set>

namespace rms {
void Plan::validate() const {
    auto require = [](bool ok, const char* why) { if (!ok) throw std::runtime_error(why); };
    require(duration_ticks <= max_tick, "plan duration exceeds tick limit");
    require(!tempos.empty() && tempos.front().tick == 0, "tempo at tick zero required");
    require(!parts.empty() && parts.size() <= 32, "plan requires 1..32 parts");
    size_t count = tempos.size();
    uint64_t previous = 0;
    for (size_t i = 0; i < tempos.size(); ++i) {
        auto t = tempos[i];
        require(t.tick <= duration_ticks && (!i || t.tick > previous), "tempos must be ordered and unique");
        // Restrict pathological tempo values before any float-to-integer conversion.
        require(std::isfinite(t.bpm) && t.bpm > 0 && t.bpm <= 60000, "tempo bpm must be finite and in (0,60000]");
        previous = t.tick;
    }
    std::unordered_set<std::string> ids;
    for (const auto& p : parts) {
        require(!p.id.empty() && p.id.size() <= 256 && p.id.find('\0') == std::string::npos,
                "part id must contain 1..256 non-NUL bytes");
        require(ids.insert(p.id).second, "duplicate part id");
        require(p.volume <= 127 && p.program <= 127, "invalid part mix");
        count += p.notes.size();
        require(count <= max_items, "too many notes and tempos");
        uint64_t previous_start = 0;
        uint64_t ends[128]{};
        for (const auto& n : p.notes) {
            require(n.start_tick >= previous_start, "notes must be ordered by start_tick");
            require(n.start_tick <= duration_ticks && n.duration_ticks > 0 &&
                    n.duration_ticks <= duration_ticks - n.start_tick, "invalid note timing");
            require(n.pitch <= 127 && n.velocity >= 1 && n.velocity <= 127, "invalid note pitch/velocity");
            require(n.start_tick >= ends[n.pitch], "overlapping unisons must be merged");
            ends[n.pitch] = n.start_tick + n.duration_ticks;
            previous_start = n.start_tick;
        }
    }
    require(count <= max_items, "too many notes and tempos");
}
Timeline::Timeline(const Plan& p) {
    p.validate();
    double seconds = 0;
    for (auto t : p.tempos) {
        if (!points_.empty()) {
            const auto& last = points_.back();
            seconds += (t.tick - last.tick) * last.seconds_per_tick;
        }
        points_.push_back({t.tick, seconds, 60.0 / (t.bpm * 960.0)});
    }
    if (!std::isfinite(this->seconds(p.duration_ticks)) || this->seconds(p.duration_ticks) > 864000)
        throw std::runtime_error("plan duration exceeds 864000 base seconds");
}
double Timeline::seconds(uint64_t tick) const noexcept {
    auto it = std::upper_bound(points_.begin(), points_.end(), tick,
        [](uint64_t t, const Point& p) { return t < p.tick; });
    const auto& p = *--it;
    return p.seconds + (tick - p.tick) * p.seconds_per_tick;
}
uint64_t Timeline::tick(double seconds) const noexcept {
    if (!std::isfinite(seconds) || seconds <= 0) return 0;
    auto it = std::upper_bound(points_.begin(), points_.end(), seconds,
        [](double t, const Point& p) { return t < p.seconds; });
    const auto& p = *--it;
    double t = p.tick + (seconds - p.seconds) / p.seconds_per_tick;
    return static_cast<uint64_t>(std::min<double>(max_tick, std::max(0.0, t + 1e-7)));
}
const char* state_name(State s) noexcept {
    switch (s) {
        case State::playing: return "playing";
        case State::paused: return "paused";
        case State::finished: return "finished";
        default: return "stopped";
    }
}
Engine::Engine(Plan p, uint32_t rate, uint32_t capacity, const std::vector<Instrument*>& instruments)
    : plan_(std::move(p)), timeline_(plan_), instruments_(instruments),
      scratch_l_(capacity), scratch_r_(capacity), rate_(rate), capacity_(capacity),
      duration_(timeline_.seconds(plan_.duration_ticks)), gain_step_(1.0 / (0.01 * rate)) {
    if (rate < 8000 || rate > 384000 || !capacity || instruments.size() != plan_.parts.size())
        throw std::runtime_error("invalid engine preparation");
    for (size_t i = 0; i < plan_.parts.size(); ++i) {
        if (!instruments_[i]) throw std::runtime_error("null instrument");
        const auto& part = plan_.parts[i];
        instruments_[i]->configure(part.volume, part.program);
        for (auto n : part.notes) {
            events_.push_back({timeline_.seconds(n.start_tick), static_cast<uint8_t>(i), n.pitch, n.velocity, true});
            events_.push_back({timeline_.seconds(n.start_tick + n.duration_ticks), static_cast<uint8_t>(i), n.pitch, 0, false});
        }
    }
    std::sort(events_.begin(), events_.end(), [](const Event& a, const Event& b) {
        if (a.seconds != b.seconds) return a.seconds < b.seconds;
        if (a.on != b.on) return !a.on; // release before retrigger at the same sample
        if (a.part != b.part) return a.part < b.part;
        return a.pitch < b.pitch;
    });
}
void Engine::silence() noexcept { for (auto* i : instruments_) i->silence(); }
void Engine::chase(size_t part) noexcept {
    if (!plan_.parts[part].enabled) return;
    for (auto n : plan_.parts[part].notes) {
        const double start = timeline_.seconds(n.start_tick);
        if (start >= position_) break;
        if (timeline_.seconds(n.start_tick + n.duration_ticks) > position_)
            instruments_[part]->note_on(n.pitch, n.velocity);
    }
}
void Engine::relocate(double position, bool reset_history) noexcept {
    position_ = std::clamp(position, 0.0, duration_);
    audible_ = position_;
    cursor_ = static_cast<size_t>(std::lower_bound(events_.begin(), events_.end(), position_,
        [](const Event& e, double s) { return e.seconds < s; }) - events_.begin());
    if (reset_history) {
        history_count_ = history_next_ = 0;
        history_floor_ = clock_;
    }
}
void Engine::play() noexcept {
    if (state_ == State::playing) return;
    if (position_ >= duration_) relocate(0, true);
    state_ = State::playing;
    for (size_t i = 0; i < instruments_.size(); ++i) chase(i);
}
void Engine::pause() noexcept {
    silence();
    state_ = State::paused;
    relocate(position_, true);
}
void Engine::stop() noexcept {
    silence();
    state_ = State::stopped;
    relocate(0, true);
}
void Engine::seek(double delta) noexcept {
    if (!std::isfinite(delta)) return;
    silence();
    relocate(position_ + delta * scale_, true);
    if (state_ == State::finished) state_ = State::paused;
    if (state_ == State::playing)
        for (size_t i = 0; i < instruments_.size(); ++i) chase(i);
}
void Engine::tempo(double scale) noexcept {
    if (std::isfinite(scale) && scale >= 0.1 && scale <= 4) scale_ = scale;
}
void Engine::mix(size_t part, bool enabled, uint8_t volume, uint8_t program) noexcept {
    if (part >= instruments_.size() || volume > 127 || program > 127) return;
    auto& p = plan_.parts[part];
    bool rechase = p.enabled != enabled || p.program != program || (p.volume == 0 && volume > 0);
    if (rechase) instruments_[part]->silence();
    p.enabled = enabled;
    p.volume = volume;
    p.program = program;
    instruments_[part]->configure(volume, program);
    if (rechase && state_ == State::playing) chase(part);
}
void Engine::master(double percent) noexcept {
    if (std::isfinite(percent) && percent >= 0 && percent <= 100) gain_target_ = percent / 100;
}
bool Engine::history(uint64_t frame, double position, double scale) noexcept {
    if (!mapping_valid_) return false;
    if (history_count_) {
        const auto& last = history_[(history_next_ + history_capacity_ - 1) % history_capacity_];
        if (last.scale == scale) return true;
    }
    if (history_count_ == history_capacity_) {
        // Keep the anchor preceding the entire supported latency horizon. A
        // later latency increase up to ten seconds must remain representable.
        const auto& second = history_[(history_next_ + 1) % history_capacity_];
        uint64_t cutoff = frame > uint64_t(rate_) * 10 ? frame - uint64_t(rate_) * 10 : 0;
        if (second.frame > cutoff) { mapping_valid_ = false; return false; }
        --history_count_;
    }
    history_[history_next_] = {frame, position, scale};
    history_next_ = (history_next_ + 1) % history_capacity_;
    ++history_count_;
    return true;
}
void Engine::update_audible(uint64_t latency) noexcept {
    if (!history_count_ || latency > uint64_t(rate_) * 10) { mapping_valid_ = false; return; }
    uint64_t target = std::max(history_floor_, clock_ > latency ? clock_ - latency : uint64_t{0});
    const auto& oldest = history_[(history_next_ + history_capacity_ - history_count_) % history_capacity_];
    if (target < oldest.frame) { mapping_valid_ = false; return; }
    for (size_t n = 0; n < history_count_; ++n) {
        const auto& h = history_[(history_next_ + history_capacity_ - 1 - n) % history_capacity_];
        if (h.frame <= target) {
            audible_ = std::clamp(h.position + (target - h.frame) * h.scale / rate_, 0.0, duration_);
            if (duration_ - audible_ < 1e-12) audible_ = duration_;
            return;
        }
    }
    mapping_valid_ = false;
}
void Engine::hold(uint64_t frames) noexcept {
    if (!frames) return;
    if (!history(clock_, position_, 0)) return;
    clock_ += frames;
}
void Engine::advance_gap(uint64_t frames) noexcept {
    if (!frames) return;
    if (state_ != State::playing) { hold(frames); return; }
    if (!history(clock_, position_, position_ < duration_ ? scale_ : 0)) return;
    if (position_ < duration_) {
        double until_end = (duration_ - position_) * rate_ / scale_;
        if (until_end <= static_cast<double>(frames)) {
            uint64_t end = clock_ + static_cast<uint64_t>(std::ceil(until_end - 1e-7));
            if (!history(end, duration_, 0)) return;
        }
    }
    silence();
    relocate(position_ + static_cast<double>(frames) * scale_ / rate_, false);
    clock_ += frames;
    for (size_t i = 0; i < instruments_.size(); ++i) chase(i);
}
void Engine::dispatch() noexcept {
    while (cursor_ < events_.size() && events_[cursor_].seconds <= position_ + 1e-10) {
        const auto& e = events_[cursor_++];
        if (!plan_.parts[e.part].enabled) continue;
        if (e.on) instruments_[e.part]->note_on(e.pitch, e.velocity);
        else instruments_[e.part]->note_off(e.pitch);
    }
}
void Engine::render(float* left, float* right, uint32_t offset, uint32_t count) noexcept {
    for (size_t p = 0; p < instruments_.size(); ++p) {
        if (!plan_.parts[p].enabled) continue;
        instruments_[p]->render(scratch_l_.data(), scratch_r_.data(), count);
        for (uint32_t i = 0; i < count; ++i) {
            left[offset + i] += std::isfinite(scratch_l_[i]) ? scratch_l_[i] : 0;
            right[offset + i] += std::isfinite(scratch_r_[i]) ? scratch_r_[i] : 0;
        }
    }
}
void Engine::process(float* left, float* right, uint32_t frames, uint64_t latency) noexcept {
    std::fill_n(left, frames, 0);
    std::fill_n(right, frames, 0);
    if (latency > uint64_t(rate_) * 10) mapping_valid_ = false;
    if (!mapping_valid_) { silence(); state_ = State::paused; return; }
    if (frames > capacity_) { silence(); state_ = State::paused; return; }
    if (state_ != State::playing) { hold(frames); return; }
    if (!history(clock_, position_, position_ < duration_ ? scale_ : 0)) {
        silence(); state_ = State::paused; return;
    }
    uint32_t offset = 0;
    while (offset < frames && position_ < duration_) {
        dispatch();
        double next = cursor_ < events_.size() ? events_[cursor_].seconds : duration_;
        next = std::min(next, duration_);
        // Absolute base-time boundaries retain fractional conversion across blocks.
        double distance = std::max(0.0, (next - position_) * rate_ / scale_);
        uint32_t count = static_cast<uint32_t>(std::min<double>(frames - offset,
                            std::max(1.0, std::ceil(distance - 1e-7))));
        render(left, right, offset, count);
        offset += count;
        position_ = std::min(duration_, position_ + count * scale_ / rate_);
        if (duration_ - position_ < 1e-12) position_ = duration_;
    }
    if (position_ >= duration_) {
        history(clock_ + offset, duration_, 0);
        dispatch(); silence();
    }
    for (uint32_t i = 0; i < frames; ++i) {
        gain_ += std::clamp(gain_target_ - gain_, -gain_step_, gain_step_);
        const double l = left[i] * gain_, r = right[i] * gain_;
        left[i] = std::isfinite(l) ? static_cast<float>(std::clamp(l, -1.0, 1.0)) : 0;
        right[i] = std::isfinite(r) ? static_cast<float>(std::clamp(r, -1.0, 1.0)) : 0;
    }
    clock_ += frames;
    update_audible(latency);
    if (!mapping_valid_) {
        std::fill_n(left, frames, 0);
        std::fill_n(right, frames, 0);
        silence(); state_ = State::paused;
        return;
    }
    if (audible_ >= duration_ && position_ >= duration_) state_ = State::finished;
}
} // namespace rms
