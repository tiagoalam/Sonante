// SPDX-License-Identifier: GPL-2.0-or-later
// Windows-only private PCM transport; source/DSP policy remains with Sonante.
#include "SonantePcmOutputPlugin.hxx"
#include "SonantePcmRing.hxx"
#include "SonantePcmPause.hxx"
#include "output/OutputAPI.hxx"
#include "output/Error.hxx"
#include "pcm/AudioFormat.hxx"
#include <array>
#include <charconv>
#include <chrono>
#include <stdexcept>
#include <string_view>
#include <utility>
#include <windows.h>
namespace {
using namespace SonantePcm;
[[noreturn]] void Fail(){throw std::runtime_error("SONANTE_PCM_CONTINUITY");}
std::uint64_t Number(const char *s) {
    const std::string_view v(s);std::uint64_t n=0;
    if(v.empty()||v.size()>20) throw std::runtime_error("SONANTE_PCM_BINDING");
    const auto result=std::from_chars(v.data(),v.data()+v.size(),n);
    if(result.ec!=std::errc{}||result.ptr!=v.data()+v.size()) throw std::runtime_error("SONANTE_PCM_BINDING");
    return n;
}
struct Inputs {
    std::array<HANDLE,4> h{};
    Inputs()=default;Inputs(const Inputs &)=delete;
    Inputs(Inputs &&other) noexcept:h(std::exchange(other.h,std::array<HANDLE,4>{})){}
    ~Inputs(){for(const auto handle:h)if(handle!=nullptr)CloseHandle(handle);}
    static Inputs Open(const ConfigBlock &block) {
        constexpr std::array names{"sonante_mapping_handle","sonante_readable_handle","sonante_writable_handle","sonante_stop_handle"};
        std::array<HANDLE,4> raw{};
        for(std::size_t i=0;i<4;++i){const auto value=Number(block.GetBlockValue(names[i],""));
            if(value==0||value>INDEX_MAX) throw std::runtime_error("SONANTE_PCM_BINDING");
            raw[i]=reinterpret_cast<HANDLE>(static_cast<std::uintptr_t>(value));
            for(std::size_t j=0;j<i;++j)if(raw[i]==raw[j])throw std::runtime_error("SONANTE_PCM_BINDING");
        }
        // Take ownership only after rejecting duplicate/pseudo handles.
        Inputs owned;owned.h=raw;
        for(const auto handle:raw){DWORD flags=0;if(!GetHandleInformation(handle,&flags)||!SetHandleInformation(handle,HANDLE_FLAG_INHERIT,0))throw std::runtime_error("SONANTE_PCM_BINDING");}
        return owned;
    }
};
struct View {
    Header *p=nullptr;
    ~View(){if(p!=nullptr)UnmapViewOfFile(p);}
};
class PrivatePcmOutput final:public AudioOutput {
    Binding binding;
    Inputs inputs;
    View view;
    std::atomic_bool interrupted{false},opened{false};
    PauseClock pause_clock;
    std::atomic_bool completed_drain{false};
    bool FullyConsumedEof() const noexcept {
        return view.p!=nullptr && Valid(*view.p,binding) &&
            Load(view.p->flags)==EOF_FLAG && Load(view.p->read)==Load(view.p->written) &&
            Load(view.p->written)<=INDEX_MAX;
    }
    void Notify() noexcept {SetEvent(inputs.h[1]);SetEvent(inputs.h[2]);}
    void Lost() noexcept {
        if(view.p!=nullptr&&Valid(*view.p,binding))Signal(*view.p,LOST);
        SetEvent(inputs.h[3]);Notify();
    }
    bool Healthy() const noexcept {return view.p!=nullptr&&Valid(*view.p,binding)&&(Load(view.p->flags)&(LOST|STOP|EOF_FLAG))==0;}
public:
    explicit PrivatePcmOutput(const ConfigBlock &block):
        AudioOutput(FLAG_PAUSE|FLAG_NEED_FULLY_DEFINED_AUDIO_FORMAT),
        binding{0,Number(block.GetBlockValue("sonante_nonce0","")),Number(block.GetBlockValue("sonante_nonce1",""))},inputs(Inputs::Open(block)) {
        static_assert(sizeof(std::uintptr_t)==8);
        const auto owner=Number(block.GetBlockValue("sonante_owner_pid",""));
        if(owner==0||owner>std::numeric_limits<std::uint32_t>::max())throw std::runtime_error("SONANTE_PCM_BINDING");
        binding.owner=static_cast<std::uint32_t>(owner);
        view.p=static_cast<Header *>(MapViewOfFile(inputs.h[0],FILE_MAP_ALL_ACCESS,0,0,sizeof(Header)));
        if(view.p==nullptr||!Healthy())throw std::runtime_error("SONANTE_PCM_BINDING");
    }
    static AudioOutput *Create(EventLoop &,const ConfigBlock &block){return new PrivatePcmOutput(block);}
    void Open(AudioFormat &f) override {
        if(f.sample_rate!=RATE||f.channels!=CHANNELS||f.format!=SampleFormat::S32)throw std::runtime_error("SONANTE_PCM_FORMAT");
        if(!Healthy()||Load(view.p->consumer)!=1||!pause_clock.Play(Load(view.p->flags)))Fail();interrupted=false;completed_drain=false;opened=true;
    }
    void Close() noexcept override {
        pause_clock.Close();
        if(opened.exchange(false)&&view.p!=nullptr&&Valid(*view.p,binding))Signal(*view.p,EOF_FLAG);
        Notify();
    }
    void Interrupt() noexcept override {interrupted=true;SetEvent(inputs.h[2]);}
    bool Pause() override {
        if(view.p==nullptr||!Valid(*view.p,binding)||!pause_clock.Pause(Load(view.p->flags))){Lost();Fail();}
        interrupted=false;return true;
    }
    std::chrono::steady_clock::duration Delay() const noexcept override {return pause_clock.Delay();}
    void Cancel() noexcept override {
        // Reset/seek cannot race a consumer cursor. The host must prepare a
        // fresh generation; never discard/replay PCM while claiming continuity.
        interrupted=false;
        // MPD RELEASE closes via Cancel even after successful Drain. No bytes
        // remain and no new Play occurred: publish terminal EOF, not LOST.
        // This never resets counters/claim or permits reuse of this generation.
        if (FullyConsumedEof() || (completed_drain.exchange(false) && Healthy() &&
                Load(view.p->read)==Load(view.p->written) && Load(view.p->written)<=INDEX_MAX)) {
            opened=false; pause_clock.Close(); Signal(*view.p,EOF_FLAG); Notify(); return;
        }
        completed_drain=false;Lost();
    }
    std::size_t Play(std::span<const std::byte> src) override {
        if(!opened||!Healthy()||!pause_clock.Play(Load(view.p->flags)))Fail();
        if(src.empty())return 0;
        completed_drain=false;
        const auto deadline=std::chrono::steady_clock::now()+std::chrono::milliseconds(250);
        while(true){
            if(interrupted.exchange(false))throw AudioOutputInterrupted{};
            const auto count=Write(*view.p,binding,src);
            if(count>0){if(!SetEvent(inputs.h[1])){Lost();Fail();}return static_cast<std::size_t>(count);}
            if(count<0){Lost();Fail();}
            const auto remaining=std::chrono::duration_cast<std::chrono::milliseconds>(deadline-std::chrono::steady_clock::now()).count();
            if(remaining<=0){Lost();Fail();}
            const std::array handles{inputs.h[3],inputs.h[2]};
            const auto result=WaitForMultipleObjects(2,handles.data(),FALSE,static_cast<DWORD>(remaining));
            if(result==WAIT_OBJECT_0||result==WAIT_TIMEOUT||result==WAIT_FAILED){Lost();Fail();}
            if(result!=WAIT_OBJECT_0+1){Lost();Fail();}
        }
    }
    void Drain() override {
        const auto deadline=std::chrono::steady_clock::now()+std::chrono::milliseconds(250);
        while(true){
            if(interrupted.exchange(false))throw AudioOutputInterrupted{};
            if(FullyConsumedEof()){completed_drain=true;return;}
            if(!Healthy()){Lost();Fail();}
            const auto read=Load(view.p->read),written=Load(view.p->written);
            if(read>written||written-read>CAPACITY){Lost();Fail();}
            if(read==written){completed_drain=true;return;}
            if(std::chrono::steady_clock::now()>=deadline){Lost();Fail();}
            const std::array handles{inputs.h[3],inputs.h[2]};
            const auto result=WaitForMultipleObjects(2,handles.data(),FALSE,10);
            if(result==WAIT_OBJECT_0||result==WAIT_FAILED){Lost();Fail();}
        }
    }
    std::map<std::string,std::string,std::less<>> GetAttributes() const noexcept override {
        if(view.p==nullptr||!Valid(*view.p,binding))return{{"sonante_pcm_state","invalid"}};
        const auto read=Load(view.p->read),written=Load(view.p->written),after=Load(view.p->read);
        const auto flags=Load(view.p->flags);
        std::map<std::string,std::string,std::less<>> result{{"sonante_pcm_abi","1"},{"sonante_pcm_format","48000:32:2"},{"sonante_pcm_state",(flags&(LOST|STOP))?"lost":(flags&EOF_FLAG)?"eof":opened?"open":"ready"}};
        if(read==after&&read<=written&&written-read<=CAPACITY){result["sonante_pcm_written_frames"]=std::to_string(written);result["sonante_pcm_read_frames"]=std::to_string(read);}
        return result;
    }
};
}
const struct AudioOutputPlugin sonante_pcm_output_plugin={"sonante_private_pcm",nullptr,&PrivatePcmOutput::Create,nullptr};
