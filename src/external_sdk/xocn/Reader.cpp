// SPDX-License-Identifier: GPL-3.0-or-later

#include <mdk/FrameReader.h>
#include <mdk/MediaInfo.h>
#include <Windows.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <condition_variable>
#include <filesystem>
#include <fstream>
#include <mutex>
#include <optional>
#include <thread>

#ifndef _M_X64
#error The X-OCN reader requires the Windows x64 Sony RAW SDK.
#endif

namespace {
using namespace mdk;
using Bytes = std::vector<uint8_t>;
constexpr uint32_t AudioRate = 48000;
constexpr std::string_view ReaderSuffix = ".gyroflow-xocn";

struct Element {
    uint64_t offset = 0;
    uint32_t size = 0;
};

uint32_t bigEndian(const uint8_t* p, size_t n) {
    uint32_t value = 0;
    for (size_t i = 0; i < n; ++i)
        value = (value << 8) | p[i];
    return value;
}

bool read(std::ifstream& file, Element element, uint8_t* out) {
    return bool(file.seekg(element.offset).read(reinterpret_cast<char*>(out), element.size));
}

struct Clip {
    struct Frame {
        Element image;
        std::array<Element, 2> audio; // the first two mono tracks, presented as stereo
        uint64_t audioStart = 0;
    };
    std::vector<Frame> frames;
    uint32_t width = 0, height = 0, storedWidth = 0, storedHeight = 0;
    uint32_t cropX = 0, cropY = 0, rate = 0, scale = 0;
    uint64_t samples = 0;
    bool hasAudio = false;

    double frameSeconds() const { return double(scale) / rate; }

    bool parseDescriptor(const Bytes& descriptor) {
        for (size_t pos = 0; pos + 4 <= descriptor.size();) {
            const auto tag = bigEndian(&descriptor[pos], 2);
            const auto length = bigEndian(&descriptor[pos + 2], 2);
            pos += 4;
            if (length > descriptor.size() - pos)
                return false;
            const auto* value = descriptor.data() + pos;
            if (length == 4) {
                auto number = bigEndian(value, 4);
                switch (tag) {
                case 0x3202: storedHeight = number; break;
                case 0x3203: storedWidth = number; break;
                case 0x3208: height = number; break;
                case 0x3209: width = number; break;
                case 0x320a: cropX = number; break;
                case 0x320b: cropY = number; break;
                }
            } else if (tag == 0x3001 && length == 8) {
                rate = bigEndian(value, 4);
                scale = bigEndian(value + 4, 4);
            }
            pos += length;
        }
        return true;
    }

    bool validGeometry() const {
        return width > 0 && height > 0 && storedWidth <= 8192 && storedHeight <= 8192
            && width <= storedWidth && height <= storedHeight
            && cropX <= storedWidth - width && cropY <= storedHeight - height
            && (storedWidth | storedHeight | cropX | cropY | width | height) % 2 == 0
            && rate > 0 && scale > 0 && rate / double(scale) <= 240;
    }

    bool index(const std::filesystem::path& path, bool probe = false) {
        std::ifstream file(path, std::ios::binary | std::ios::ate);
        if (!file)
            return false;
        const auto size = uint64_t(file.tellg());
        uint64_t pos = 0;
        while (pos < size) {
            std::array<uint8_t, 17> header{};
            if (!read(file, {pos, uint32_t(header.size())}, header.data())
                || bigEndian(header.data(), 4) != 0x060e2b34)
                return false;
            pos += header.size();
            uint64_t length = header[16];
            if (length & 0x80) {
                auto bytes = unsigned(length & 0x7f);
                std::array<uint8_t, 8> count{};
                if (!bytes || bytes > count.size() || !read(file, {pos, bytes}, count.data()))
                    return false;
                length = 0;
                for (unsigned i = 0; i < bytes; ++i)
                    length = (length << 8) | count[i];
                pos += bytes;
            }
            if (pos > size || length > size - pos)
                return false;

            if (bigEndian(header.data() + 4, 4) == 0x02530101
                && bigEndian(header.data() + 8, 4) == 0x0d010101 && header[14] == 0x27
                && frames.empty() && length <= 64 * 1024) {
                Bytes descriptor(static_cast<size_t>(length));
                if (!read(file, {pos, uint32_t(length)}, descriptor.data()) || !parseDescriptor(descriptor))
                    return false;
            } else if (bigEndian(header.data() + 4, 4) == 0x01020106
                       && bigEndian(header.data() + 8, 4) == 0x0e060d03 && header[12] == 0x19) {
                if (length < 312 || length > 64 * 1024 * 1024)
                    return false;
                std::array<uint8_t, 4> magic{};
                if (!read(file, {pos, uint32_t(magic.size())}, magic.data())
                    || bigEndian(magic.data(), 4) != 0x53425268) // FX5 X-OCN bitstream family H
                    return false;
                if (probe)
                    return validGeometry();
                frames.push_back({{pos, uint32_t(length)}});
            } else if (!frames.empty() && bigEndian(header.data() + 8, 4) == 0x0d010301
                       && header[12] == 0x16 && header[13] == 0x04 && header[14] == 0x03
                       && header[15] < 2 && length && length <= 128 * 1024 && length % 3 == 0) {
                frames.back().audio[header[15]] = {pos, uint32_t(length)};
            }
            pos += length;
            if (probe && pos > 4 * 1024 * 1024)
                return false;
        }
        if (!validGeometry() || frames.empty())
            return false;

        hasAudio = std::all_of(frames.begin(), frames.end(), [&](Frame& frame) {
            frame.audioStart = samples;
            samples += frame.audio[0].size / 3;
            return frame.audio[0].size > 0 && frame.audio[0].size == frame.audio[1].size;
        });
        // Accept only the 48 kHz, frame-wrapped PCM layout used by FX5. Do not present a mismatched audio timeline.
        hasAudio = hasAudio && std::abs(double(samples) / AudioRate - frames.size() * frameSeconds()) < frameSeconds();
        return true;
    }
};

std::filesystem::path configuredSdk() {
    wchar_t path[32768];
    const auto size = GetEnvironmentVariableW(L"GYROFLOW_SONY_RAW_DLL", path, DWORD(std::size(path)));
    if (size)
        return size < std::size(path) ? std::filesystem::path(path) : std::filesystem::path{};

    const auto length = GetModuleFileNameW(nullptr, path, DWORD(std::size(path)));
    if (!length || length >= std::size(path))
        return {};
    const auto config = std::filesystem::path(path).parent_path() / L"SonyRawDev.path";
    std::ifstream file(config, std::ios::binary);
    std::string line;
    if (!std::getline(file, line))
        return {};
    if (!line.empty() && line.back() == '\r')
        line.pop_back();
    std::string_view utf8(line);
    if (utf8.starts_with("\xEF\xBB\xBF"))
        utf8.remove_prefix(3);
    return std::filesystem::path(std::u8string(utf8.begin(), utf8.end()));
}

// Sony RAW Development SDK 6 on Windows x64. The layout follows RawDevControl6 and
// RawDevBuffer as used by Dec18studios/fx5-xocn-tools (MIT). The SDK is supplied locally.
class Decoder {
public:
    ~Decoder() {
        if (context_)
            destroy_(context_);
        if (dll_)
            FreeLibrary(dll_);
    }

    bool open(const Clip& clip, const Bytes& firstFrame) {
        const auto path = configuredSdk();
        if (!path.is_absolute())
            return false;
        dll_ = LoadLibraryExW(path.c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        if (!dll_)
            return false;
        const auto create = symbol<void*(*)()>("?create@SonyRawDev@sony_raw_dev@@SAPEAV12@XZ");
        destroy_ = symbol<void(*)(void*)>("?destroy@SonyRawDev@sony_raw_dev@@SAXPEAV12@@Z");
        const auto version = symbol<char*(*)(void*, char*, size_t)>("?getVersion@SonyRawDev@sony_raw_dev@@UEBAPEADPEAD_K@Z");
        const auto group = symbol<int(*)(void*, const void*, unsigned, unsigned*)>(
            "?getBitstreamGroup@SonyRawDev@sony_raw_dev@@UEBAHPEBXIPEAW4BITSTREAM_GROUP@2@@Z");
        const auto initialize = symbol<int(*)(void*, const void*)>("?initialize@SonyRawDev@sony_raw_dev@@UEAAHPEBURawDevControl6@2@@Z");
        develop_ = symbol<int(*)(void*, const void*, unsigned, PlaneBuffer*)>(
            "?develop@SonyRawDev@sony_raw_dev@@UEAAHPEBXIPEAURawDevBuffer@2@@Z");
        if (!create || !destroy_ || !version || !group || !initialize || !develop_)
            return false;
        context_ = create();
        if (!context_)
            return false;
        char sdkVersion[64]{};
        version(context_, sdkVersion, sizeof(sdkVersion));
        if (std::string_view(sdkVersion, 4) != "6.0.")
            return false;
        unsigned family = 0;
        if (group(context_, firstFrame.data(), unsigned(firstFrame.size()), &family) != 0 || family != 11)
            return false;
        const std::array<unsigned, 22> control = {
            0x6c6c7663, family, clip.storedWidth, clip.storedHeight, clip.storedWidth, clip.storedHeight, 0x100
        };
        if (initialize(context_, control.data()) != 0)
            return false;
        const size_t pixels = size_t(clip.storedWidth) * clip.storedHeight;
        planes_[0].resize(pixels / 4);
        planes_[1].resize(pixels / 2);
        planes_[2].resize(pixels / 4);
        return true;
    }

    VideoFrame decode(const Clip& clip, const Bytes& payload) {
        PlaneBuffer output{};
        for (size_t i = 0; i < planes_.size(); ++i) {
            output.data[i] = planes_[i].data();
            output.size[i] = unsigned(planes_[i].size() * sizeof(uint16_t));
        }
        if (develop_(context_, payload.data(), unsigned(payload.size()), &output) != 0)
            return {};

        // One output pixel per RGGB cell for preview. MediaInfo reports the active sensor raster.
        VideoFrame frame(clip.width / 2, clip.height / 2, PixelFormat::BGRX);
        frame.setBuffers(nullptr);
        const auto buffer = frame.buffer();
        const auto stride = clip.storedWidth / 2;
        for (uint32_t y = 0; y < clip.height / 2; ++y) {
            auto* dst = reinterpret_cast<uint8_t*>(buffer->data()) + y * buffer->stride();
            auto row = size_t(y + clip.cropY / 2) * stride + clip.cropX / 2;
            for (uint32_t x = 0; x < clip.width / 2; ++x) {
                const size_t cell = row + x;
                dst[4 * x] = previewCurve_[planes_[2][cell]];
                dst[4 * x + 1] = previewCurve_[(unsigned(planes_[1][2 * cell]) + planes_[1][2 * cell + 1]) / 2];
                dst[4 * x + 2] = previewCurve_[planes_[0][cell]];
                dst[4 * x + 3] = 255;
            }
        }
        return frame;
    }

private:
    struct PlaneBuffer {
        void* data[3];
        unsigned size[3];
    };
    template<typename T> T symbol(const char* name) const {
        return reinterpret_cast<T>(GetProcAddress(dll_, name));
    }
    HMODULE dll_ = nullptr;
    void* context_ = nullptr;
    void (*destroy_)(void*) = nullptr;
    int (*develop_)(void*, const void*, unsigned, PlaneBuffer*) = nullptr;
    std::array<std::vector<uint16_t>, 3> planes_;
    // FX5 raw codes 512 and 5472 correspond to sensor black and 90% white.
    // Apply an SDR transfer for stabilization preview in camera-native primaries.
    inline static const auto previewCurve_ = [] {
        std::array<uint8_t, 65536> curve{};
        for (size_t code = 0; code < curve.size(); ++code) {
            const auto linear = std::clamp((double(code) - 512.) * 0.9 / 4960., 0., 1.);
            const auto srgb = linear < 0.0031308 ? linear * 12.92 : 1.055 * std::pow(linear, 1. / 2.4) - 0.055;
            curve[code] = static_cast<uint8_t>(std::round(srgb * 255));
        }
        return curve;
    }();
};

std::filesystem::path mediaPath(std::string_view utf8) {
    if (!utf8.ends_with(ReaderSuffix))
        return {};
    utf8.remove_suffix(ReaderSuffix.size());
    const auto* s = reinterpret_cast<const char8_t*>(utf8.data());
    return std::filesystem::path(std::u8string(s, utf8.size()));
}

class Reader final : public FrameReader {
public:
    ~Reader() override { join(); }
    const char* name() const override { return "XOCN"; }
    void setTimeout(int64_t, TimeoutCallback) override {}
    int64_t buffered(int64_t*, float*) const override { return 0; }

    bool load() override {
        join();
        try {
            path_ = mediaPath(url());
            clip_ = {};
            if (!clip_.index(path_))
                return false;
            std::ifstream file(path_, std::ios::binary);
            const auto image = clip_.frames.front().image;
            Bytes firstFrame(image.size);
            auto decoder = std::make_unique<Decoder>();
            if (!read(file, image, firstFrame.data()) || !decoder->open(clip_, firstFrame))
                return false;

            const auto duration = std::llround(clip_.frames.size() * clip_.frameSeconds() * 1000);
            MediaInfo info;
            info.format = "mxf";
            info.duration = duration;
            info.streams = 1;
            auto& video = info.video.emplace_back();
            video.index = 0;
            video.frames = clip_.frames.size();
            video.duration = duration;
            video.codec.codec = "Sony X-OCN";
            video.codec.width = clip_.width;
            video.codec.height = clip_.height;
            video.codec.frame_rate = 1. / clip_.frameSeconds();
            video.codec.format = PixelFormat::BGRX;
            video.codec.bits_per_raw_sample = 16;
            if (clip_.hasAudio) {
                auto& audio = info.audio.emplace_back();
                audio.index = 1;
                audio.frames = clip_.samples;
                audio.duration = std::llround(1000. * clip_.samples / AudioRate);
                audio.codec.codec = "pcm_s24le";
                audio.codec.format = AudioFormat::SampleFormat::S32;
                audio.codec.channel_layout = audioFormat_.channelMap();
                audio.codec.channels = 2;
                audio.codec.sample_rate = AudioRate;
                audio.codec.bits_per_raw_sample = 24;
                info.streams++;
            }
            {
                const std::lock_guard lock(mutex_);
                stopping_ = false;
                pendingVideo_ = Request{};
                pendingAudio_ = Request{};
            }
            currentFrame_ = 0;
            changed(info);
            update(MediaStatus::Loaded);
            if (state() == State::Stopped)
                update(State::Running);
            videoThread_ = std::thread([this, decoder = std::move(decoder)] { videoLoop(*decoder); });
            if (clip_.hasAudio && !activeTracks(MediaType::Audio).empty())
                audioThread_ = std::thread([this] { audioLoop(); });
            return true;
        } catch (...) {
            unload();
            return false;
        }
    }

    bool unload() override {
        {
            const std::lock_guard lock(mutex_);
            stopping_ = true;
        }
        ready_.notify_all();
        update(MediaStatus::Unloaded);
        update(State::Stopped);
        // The player can stop from a frame callback running on one of the workers.
        // Join from the next load or the destructor, after the MDK queues have been released.
        return true;
    }

protected:
    bool seekTo(int64_t ms, SeekFlag flags, int id) override {
        std::unique_lock lock(mutex_);
        if (stopping_ || clip_.frames.empty())
            return false;
        const auto last = int64_t(clip_.frames.size() - 1);
        const auto relative = test_flag(flags, SeekFlag::FromNow | SeekFlag::Frame);
        const auto index = relative
            ? currentFrame_.load() + std::clamp(ms, -last, last)
            : std::llround(std::clamp(double(ms) / (1000 * clip_.frameSeconds()), 0., double(last)));
        const Request target{size_t(std::clamp(index, int64_t(0), last)), id,
                             !test_flag(flags & SeekFlag::IOCompleteCallback)};
        pendingVideo_ = target;
        pendingAudio_ = target;
        lock.unlock();
        if (!target.waitForDecode)
            seekComplete(timeMs(target.index), id);
        ready_.notify_all();
        return true;
    }

private:
    struct Request {
        size_t index = 0;
        int id = -1;
        bool waitForDecode = true;
    };

    int64_t timeMs(size_t index) const { return std::llround(index * clip_.frameSeconds() * 1000); }

    void join() {
        if (videoThread_.joinable())
            videoThread_.join();
        if (audioThread_.joinable())
            audioThread_.join();
    }

    template<typename Render>
    void run(std::optional<Request>& pending, Render render) {
        std::optional<Request> next;
        while (true) {
            {
                std::unique_lock lock(mutex_);
                ready_.wait(lock, [&] { return stopping_ || pending || next; });
                if (stopping_)
                    return;
                if (pending)
                    next = std::exchange(pending, std::nullopt);
            }
            const auto target = *next;
            next.reset();
            if (render(target) && target.index + 1 < clip_.frames.size())
                next = Request{target.index + 1};
        }
    }

    void videoLoop(Decoder& decoder) {
        std::ifstream file(path_, std::ios::binary);
        Bytes payload;
        bool prepared = false;
        run(pendingVideo_, [&](const Request& target) {
            const auto item = clip_.frames[target.index].image;
            payload.resize(item.size);
            auto frame = read(file, item, payload.data()) ? decoder.decode(clip_, payload) : VideoFrame{};
            if (!frame) {
                if (target.id >= 0 && target.waitForDecode)
                    seekComplete(-1, target.id);
                update(MediaStatus::Invalid);
                frameAvailable(VideoFrame().setTimestamp(TimestampEOS));
                return false;
            }
            currentFrame_ = target.index;
            frame.setTimestamp(target.index * clip_.frameSeconds());
            frame.setDuration(clip_.frameSeconds());
            // The desktop player publishes dimensions only once the reader is prepared.
            // Send that status before the first frame can trigger the renderer's size event.
            if (!prepared) {
                if (!test_flag(mediaStatus() & MediaStatus::Loaded))
                    return false;
                prepared = true;
                update(mediaStatus() | MediaStatus::Prepared);
            }
            if (target.id >= 0) {
                if (target.waitForDecode)
                    seekComplete(timeMs(target.index), target.id);
                frameAvailable(VideoFrame(frame.format()).setTimestamp(frame.timestamp()));
            }
            if (!frameAvailable(frame))
                return false;
            if (target.index + 1 == clip_.frames.size()) {
                update(mediaStatus() | MediaStatus::End);
                frameAvailable(VideoFrame().setTimestamp(TimestampEOS));
            }
            return true;
        });
    }

    void audioLoop() {
        std::ifstream file(path_, std::ios::binary);
        std::array<Bytes, 2> channels;
        run(pendingAudio_, [&](const Request& target) {
            const auto& indexed = clip_.frames[target.index];
            for (size_t i = 0; i < channels.size(); ++i) {
                channels[i].resize(indexed.audio[i].size);
                if (!read(file, indexed.audio[i], channels[i].data())) {
                    frameAvailable(AudioFrame().setTimestamp(TimestampEOS));
                    return false;
                }
            }
            const int count = int(channels[0].size() / 3);
            AudioFrame frame(audioFormat_, count);
            auto* pcm = reinterpret_cast<uint32_t*>(frame.buffer()->data());
            for (int i = 0; i < count; ++i) {
                for (size_t ch = 0; ch < channels.size(); ++ch) {
                    const auto* sample = channels[ch].data() + 3 * i;
                    pcm[2 * i + ch] = uint32_t(sample[0]) << 8 | uint32_t(sample[1]) << 16 | uint32_t(sample[2]) << 24;
                }
            }
            frame.setTimestamp(double(indexed.audioStart) / AudioRate);
            if (target.id >= 0)
                frameAvailable(AudioFrame(audioFormat_).setTimestamp(frame.timestamp()));
            if (!frameAvailable(frame))
                return false;
            if (target.index + 1 == clip_.frames.size())
                frameAvailable(AudioFrame().setTimestamp(TimestampEOS));
            return true;
        });
    }

    std::filesystem::path path_;
    Clip clip_;
    const AudioFormat audioFormat_{AudioFormat::SampleFormat::S32, 2, AudioRate};
    std::mutex mutex_;
    std::condition_variable ready_;
    std::thread videoThread_, audioThread_;
    std::optional<Request> pendingVideo_, pendingAudio_;
    std::atomic<int64_t> currentFrame_{0};
    bool stopping_ = true;
};
}

extern "C" uint64_t gyroflow_xocn_probe(const wchar_t* path) {
    try {
        Clip clip;
        return clip.index(path, true) ? (uint64_t(clip.width) << 32) | clip.height : 0;
    } catch (...) {
        return 0;
    }
}

extern "C" bool gyroflow_xocn_configured() {
    try {
        const auto path = configuredSdk();
        std::error_code error;
        return path.is_absolute() && std::filesystem::is_regular_file(path, error);
    } catch (...) {
        return false;
    }
}

MDK_PLUGIN(xocn) {
    // Route all media types for this private suffix through the same reader.
    // MDK otherwise also tries opening the synthetic path as a subtitle source.
    mdk::FrameReader::registerOnce("XOCN", [] { return new Reader(); },
        {{ReaderSuffix.data()}, {ReaderSuffix.data()}, {ReaderSuffix.data()}});
    return MDK_ABI_VERSION;
}
