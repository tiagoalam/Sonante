//! Fixed-label observations of MPD's authored Direct output error lines.
//! No raw byte, path, URL, timestamp or message is retained across calls.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[repr(u8)]
pub(super) enum Hint{Open,Initialize,Format,Exclusive,Changed,Unavailable,Busy}
pub(super) const ALL:[Hint;7]=[Hint::Open,Hint::Initialize,Hint::Format,Hint::Exclusive,Hint::Changed,Hint::Unavailable,Hint::Busy];
impl Hint{
    pub(super) fn bit(self)->u8{1u8<<(self as u8)}
    pub(super) fn code(self)->&'static str{match self{
        Self::Open=>"WIN_DIRECT_STDIO_OPEN_FAILED",Self::Initialize=>"WIN_DIRECT_STDIO_INITIALIZE_FAILED",
        Self::Format=>"WIN_DIRECT_STDIO_SOURCE_FORMAT_UNSUPPORTED",Self::Exclusive=>"WIN_DIRECT_STDIO_EXCLUSIVE_DENIED",
        Self::Changed=>"WIN_DIRECT_STDIO_ENDPOINT_CHANGED",Self::Unavailable=>"WIN_DIRECT_STDIO_ENDPOINT_UNAVAILABLE",Self::Busy=>"WIN_DIRECT_STDIO_DEVICE_BUSY",
    }}
    fn rank(self)->u8{match self{Self::Open=>0,Self::Initialize=>1,_=>2}}
}
const PREFIXES:[&[u8];4]=[
    b"exception: Failed to open \"Sonante Windows Direct\" (wasapi); ",
    b"exception: Failed to enable output \"Sonante Windows Direct\" (wasapi); ",
    b"output: Failed to play on \"Sonante Windows Direct\" (wasapi): ",
    b"output: Failed to pause \"Sonante Windows Direct\" (wasapi): ",
];
const MARKERS:[(&[u8],Hint);7]=[
    (b"Sonante Direct strict: source format unsupported",Hint::Format),
    (b"Sonante Direct PCM negotiation: no compatible supported PCM format",Hint::Format),
    (b"Format is not supported",Hint::Format),
    (b"Exclusive mode not allowed",Hint::Exclusive),
    (b"Sonante endpoint identity changed",Hint::Changed),
    (b"Sonante endpoint unavailable",Hint::Unavailable),
    (b"Unable to initialize audio client",Hint::Initialize),
];
const SYMBOLS:[(&[u8],Hint);4]=[
    (b"AUDCLNT_E_UNSUPPORTED_FORMAT",Hint::Format),(b"AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED",Hint::Exclusive),
    (b"AUDCLNT_E_DEVICE_INVALIDATED",Hint::Unavailable),(b"AUDCLNT_E_DEVICE_IN_USE",Hint::Busy),
];
const DEAD:usize=usize::MAX;
struct Line{
    length:usize,timestamp:bool,valid:bool,cr:bool,body:bool,
    prefix:[usize;4],markers:[usize;6],hint:Hint,ambiguous:bool,
    number_state:u8,number_digits:u8,number:u32,previous_alnum:bool,
    symbols:[usize;4],word:bool,
}
impl Default for Line{fn default()->Self{Self{length:0,timestamp:false,valid:true,cr:false,body:false,prefix:[0;4],markers:[0;6],hint:Hint::Open,ambiguous:false,number_state:0,number_digits:0,number:0,previous_alnum:false,symbols:[0;4],word:false}}}
impl Line{
    fn choose(&mut self,hint:Hint){
        if self.hint.rank()==2&&hint.rank()==2&&hint!=self.hint{self.ambiguous=true;}
        if hint.rank()>self.hint.rank(){self.hint=hint;}
    }
    fn number_end(&mut self){
        if self.number_state==2&&self.number_digits==8{
            match self.number{0x88890008=>self.choose(Hint::Format),0x8889000e=>self.choose(Hint::Exclusive),0x88890004=>self.choose(Hint::Unavailable),0x8889000a=>self.choose(Hint::Busy),_=>{}}
        }
        self.number_state=0;self.number_digits=0;self.number=0;
    }
    fn word_end(&mut self){
        if self.word{for(i,(symbol,hint))in SYMBOLS.iter().enumerate(){if self.symbols[i]==symbol.len(){self.choose(*hint);}}}
        self.word=false;self.symbols=[0;4];
    }
    fn byte(&mut self,b:u8){
        self.length=self.length.saturating_add(1);
        if self.length>4096||self.cr{self.valid=false;}
        if !self.valid{return;}
        if b==b'\r'{self.cr=true;return;}
        if b.is_ascii_control(){self.valid=false;return;}
        if self.length==1{self.timestamp=b.is_ascii_digit();}
        if self.timestamp&&self.length<=20{
            let p=self.length-1;let pattern=b"0000-00-00T00:00:00 ";
            if if pattern[p]==b'0'{!b.is_ascii_digit()}else{b!=pattern[p]}{self.valid=false;}return;
        }
        if !self.body{
            for(i,pattern)in PREFIXES.iter().enumerate(){
                if self.prefix[i]==DEAD{continue;}
                if pattern[self.prefix[i]]==b{self.prefix[i]+=1;if self.prefix[i]==pattern.len(){self.body=true;}}
                else{self.prefix[i]=DEAD;}
            }
            if !self.body&&self.prefix.iter().all(|p|*p==DEAD){self.valid=false;}return;
        }
        for(i,(pattern,hint))in MARKERS.iter().enumerate(){
            let at=self.markers[i];
            self.markers[i]=if pattern[at].eq_ignore_ascii_case(&b){at+1}else{usize::from(pattern[0].eq_ignore_ascii_case(&b))};
            if self.markers[i]==pattern.len(){self.markers[i]=0;self.choose(*hint);}
        }
        if b.is_ascii_alphanumeric()||b==b'_'{
            self.word=true;
            for(i,(symbol,_))in SYMBOLS.iter().enumerate(){let at=self.symbols[i];self.symbols[i]=if at<symbol.len()&&symbol[at].eq_ignore_ascii_case(&b){at+1}else{DEAD};}
        }else{self.word_end();}
        match self.number_state{
            0=>{if b==b'0'&&!self.previous_alnum{self.number_state=1;}},
            1=>{if b==b'x'||b==b'X'{self.number_state=2;}else{self.number_end();}},
            2=>{if let Some(n)=(b as char).to_digit(16){
                self.number_digits=self.number_digits.saturating_add(1);
                if self.number_digits<=8{self.number=(self.number<<4)|n;}else{self.number_state=3;}
            }else{self.number_end();}},
            _=>{if !b.is_ascii_hexdigit(){self.number_end();}},
        }
        self.previous_alnum=b.is_ascii_alphanumeric()||b==b'_';
    }
    fn finish(&mut self)->Option<Hint>{
        self.number_end();
        self.word_end();
        (self.valid&&self.body).then_some(if self.ambiguous{Hint::Open}else{self.hint})
    }
}
#[derive(Default)]pub(super) struct Scan{line:Line,emitted:u8}
impl Scan{
    pub(super) fn push(&mut self,bytes:&[u8],mut emit:impl FnMut(Hint)){
        for b in bytes{
            if *b==b'\n'{
                if let Some(hint)=self.line.finish(){let bit=hint.bit();if self.emitted&bit==0{self.emitted|=bit;emit(hint);}}
                self.line=Line::default();
            }else{self.line.byte(*b);}
        }
    }
}
#[cfg(test)]mod tests{
    use super::*;
    fn classify(bytes:&[u8],size:usize)->Vec<Hint>{let mut scan=Scan::default();let mut out=Vec::new();for part in bytes.chunks(size){scan.push(part,|hint|out.push(hint));}out}
    fn line(message:&str)->String{format!("2026-11-05T12:13:14 exception: Failed to open \"Sonante Windows Direct\" (wasapi); {message}\n")}
    #[test]fn authored_nested_errors_survive_every_fragment_boundary_without_retaining_the_message(){
        for(message,hint)in[("Sonante Direct strict: source format unsupported",Hint::Format),("Sonante Direct PCM negotiation: no compatible supported PCM format",Hint::Format),("Exclusive mode not allowed",Hint::Exclusive),("Sonante endpoint identity changed",Hint::Changed),("Sonante endpoint unavailable",Hint::Unavailable),("Unable to initialize audio client: private translated message (0x8889000a)",Hint::Busy)]{
            let bytes=line(message);for size in 1..=bytes.len(){assert_eq!(classify(bytes.as_bytes(),size),[hint]);}
        }
        assert!(std::mem::size_of::<Scan>()<256);
    }
    #[test]fn untrusted_domains_names_timestamps_and_partial_lines_do_not_attribute_a_direct_failure(){
        for bytes in [b"decoder: Failed to open \"Sonante Windows Direct\" (wasapi); Exclusive mode not allowed\n".as_slice(),b"exception: Failed to decode https://private/Exclusive mode not allowed\n",b"exception: Failed to open \"Other endpoint\" (wasapi); Exclusive mode not allowed\n",b"2026-11-05?12:13:14 exception: Failed to open \"Sonante Windows Direct\" (wasapi); Exclusive mode not allowed\n",b"metadata exception: Failed to open \"Sonante Windows Direct\" (wasapi); Exclusive mode not allowed\n"]{assert!(classify(bytes,1).is_empty());}
        assert!(classify(line("Exclusive mode not allowed").trim_end().as_bytes(),1).is_empty());
        assert_eq!(classify(b"output: Failed to play on \"Sonante Windows Direct\" (wasapi): Sonante endpoint unavailable\r\n",1),[Hint::Unavailable]);
    }
    #[test]fn only_exact_numeric_hresult_tokens_win_over_generic_initialization(){
        for(code,hint)in[("0x88890008",Hint::Format),("0X8889000E",Hint::Exclusive),("0x88890004",Hint::Unavailable),("0x8889000A",Hint::Busy),("0x888900080",Hint::Initialize),("0x88890008f",Hint::Initialize),("0xdeadbeef",Hint::Initialize),("p0x88890008",Hint::Initialize)]{assert_eq!(classify(line(&format!("Unable to initialize audio client ({code})")).as_bytes(),1),[hint]);}
        assert_eq!(classify(line("Exclusive mode not allowed; Sonante endpoint unavailable").as_bytes(),1),[Hint::Open]);
    }
    #[test]fn upstream_symbolic_hresult_fallback_is_projected_only_as_a_whole_public_token(){
        for(symbol,hint)in SYMBOLS{
            let name=std::str::from_utf8(symbol).unwrap();
            for value in[format!("Unable to initialize audio client: {name}"),format!("Unable to initialize audio client: ({name})")]{
                assert_eq!(classify(line(&value).as_bytes(),1),[hint]);
            }
            assert_eq!(classify(line(&format!("Unable to initialize audio client: {name}_PRIVATE")).as_bytes(),1),[Hint::Initialize]);
            assert_eq!(classify(line(&format!("Unable to initialize audio client: PRIVATE_{name}")).as_bytes(),1),[Hint::Initialize]);
        }
    }
    #[test]fn overlong_control_characters_and_floods_cannot_fill_the_report_or_hide_the_next_valid_line(){
        let mut input=line(&"private payload".repeat(1000));input.push_str(&line("Sonante endpoint identity changed"));assert_eq!(classify(input.as_bytes(),317),[Hint::Changed]);
        assert_eq!(classify(line("Exclusive mode not allowed").repeat(10000).as_bytes(),4096),[Hint::Exclusive]);
        assert!(classify(line("Exclu\rsive mode not allowed").as_bytes(),1).is_empty());
        let mut bytes=line("Sonante endpoint unavailable").into_bytes();bytes.insert(bytes.len()-1,0xff);assert_eq!(classify(&bytes,1),[Hint::Unavailable]);
    }
}
