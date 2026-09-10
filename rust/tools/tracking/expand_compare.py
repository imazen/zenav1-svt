from pathlib import Path
p=Path('/home/lilith/work/zen/zenmetrics/benchmarks/av1-compare/src/lib.rs')
s=p.read_text()
s=s.replace('//! deliberately specifies 8-bit, limited-range I420 and one still per call.','//! covers limited-range stills at each backend\'s native precision and chroma.')
idx=s.index('#[derive(Clone, Copy, Debug, Deserialize, Serialize)]\n#[serde(deny_unknown_fields)]')
s=s[:idx]+'''#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Chroma {
    #[default] #[serde(rename="420")] Cs420,
    #[serde(rename="422")] Cs422,
    #[serde(rename="444")] Cs444,
    #[serde(rename="mono")] Mono,
}
impl Chroma {
    pub fn shifts(self)->(usize,usize) { match self { Self::Cs420|Self::Mono=>(1,1),Self::Cs422=>(1,0),Self::Cs444=>(0,0) } }
}
fn eight()->u8 {8}
'''+s[idx:]
s=s.replace('    pub threads: u32,','''    pub threads: u32,
    #[serde(default="eight")] pub bit_depth:u8,
    #[serde(default)] pub chroma:Chroma,
    #[serde(default)] pub tune:Option<u8>,
    #[serde(default)] pub scm:Option<u8>,
    #[serde(default)] pub sb128:bool,
''',1)
a=s.index('    pub fn validate(&self');b=s.index('    pub fn revision(&self',a)
s=s[:a]+'''    pub fn plane_lengths(&self)->(usize,usize) {
        let (sx,sy)=self.chroma.shifts();
        (self.width as usize*self.height as usize, if self.chroma==Chroma::Mono {0} else {
            (self.width as usize).div_ceil(1<<sx)*(self.height as usize).div_ceil(1<<sy) })
    }
    pub fn validate_configuration(&self)->Result<(),String> {
        if !(64..=16384).contains(&self.width) || !(64..=16384).contains(&self.height) {
            return Err("comparison protocol requires dimensions in 64..=16384".into());
        }
        if !matches!(self.bit_depth,8|10|12) {return Err("bit depth must be 8, 10 or 12".into());}
        if !(1..=16).contains(&self.threads) {return Err("thread setting must be 1..=16".into());}
        let (qmax,smax)=match self.backend {Backend::Rav1e=>(255,10),Backend::CSvt=>(63,13),_=>(63,9)};
        if self.quantizer>qmax || self.speed>smax {return Err("native quantizer or preset out of range".into());}
        if matches!(self.backend,Backend::CSvt|Backend::Svt) {
            if self.bit_depth==12 || !matches!(self.chroma,Chroma::Cs420|Chroma::Mono) {
                return Err("SVT supports 8/10-bit 420; Rust additionally supports mono".into());
            }
            if matches!(self.backend,Backend::CSvt) && (self.chroma==Chroma::Mono || !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2)) {
                return Err("C SVT requires even 420; monochrome is a Rust extension".into());
            }
        } else if self.tune.is_some() || self.scm.is_some() {return Err("explicit SVT tools require an SVT backend".into());}
        if self.tune.is_some_and(|v|v>4) || self.scm.is_some_and(|v|v>2) {return Err("SVT tune/SCM outside supported range".into());}
        if self.sb128 && !matches!(self.backend,Backend::Libaom|Backend::Aom) {return Err("explicit SB size is currently an AOM arm".into());}
        if matches!(self.backend,Backend::Aom) && self.threads!=1 {return Err("standalone Rust AOM has no threaded encoder API".into());}
        Ok(())
    }
    pub fn validate(&self,pixels:&[u8])->Result<(),String> {
        self.validate_configuration()?;
        let (y,c)=self.plane_lengths(); let size=(y+2*c)*if self.bit_depth==8 {1}else{2};
        if pixels.len()!=size {return Err(format!("expected {size} packed planar bytes, got {}",pixels.len()));}
        if self.bit_depth>8 && pixels.chunks_exact(2).any(|p|u16::from_le_bytes([p[0],p[1]]) >= (1<<self.bit_depth)) {
            return Err("input sample exceeds coded bit depth".into());
        }
        Ok(())
    }
'''+s[b:]
# Extend C API calls. Both APIs receive the same fields and independently refuse scope.
s=s.replace('        out: *mut COutput,','        bd:u32, sx:u32, sy:u32, mono:u32, tune:i32, scm:i32, sb128:u32,\n        out: *mut COutput,')
s=s.replace('    let (y, uv) = pixels.split_at(w * h);\n    let (u, v) = uv.split_at(w * h / 4);','''    let (yn,cn)=cfg.plane_lengths(); let bps=if cfg.bit_depth==8 {1}else{2};
    let (y,uv)=pixels.split_at(yn*bps); let (u,v)=uv.split_at(cn*bps);
    let samples=|| [y,u,v].map(|p| if cfg.bit_depth==8 {p.iter().map(|&v|u16::from(v)).collect::<Vec<_>>()}else{p.chunks_exact(2).map(|p|u16::from_le_bytes([p[0],p[1]])).collect()});
    let (sx,sy)=cfg.chroma.shifts();''')
s=s.replace('                    cfg.threads,\n                    &mut out,','''                    cfg.threads, cfg.bit_depth as u32,sx as u32,sy as u32,u32::from(cfg.chroma==Chroma::Mono),
                    cfg.tune.map_or(-1,i32::from),cfg.scm.map_or(-1,i32::from),u32::from(cfg.sb128),
                    &mut out,''')
a=s.index('        Backend::Svt => {',s.index('pub fn encode'));b=s.index('        Backend::Aom => {',a)
s=s[:a]+'''        Backend::Svt => {
            use svtav1::encoder::{pipeline::EncodePipeline, rate_control::{RcConfig, RcMode}};
            let rc=RcConfig {mode:RcMode::Cqp,qp:cfg.quantizer as u8,..Default::default()};
            let mut p=EncodePipeline::new(cfg.width,cfg.height,cfg.speed as u8,rc,0,1)
                .with_chroma_420(cfg.chroma==Chroma::Cs420).with_thread_count(cfg.threads as usize).with_bit_depth(cfg.bit_depth);
            if let Some(tune)=cfg.tune {p.hdr.tune=tune;}
            if let Some(scm)=cfg.scm {p.hdr.screen_content_mode=Some(scm);}
            let r=if cfg.bit_depth==8 {
                if cfg.chroma==Chroma::Mono {p.try_encode_frame(y,w)} else {p.try_encode_frame_420(y,u,v,w)}
            } else {let s=samples();if cfg.chroma==Chroma::Mono {p.try_encode_frame_hbd(&s[0],w)}else{p.try_encode_frame_420_hbd(&s[0],&s[1],&s[2],w)}};
            r.map_err(|e|e.to_string())
        }
'''+s[b:]
a=s.index('        Backend::Aom => {',s.index('pub fn encode'));b=s.index('        Backend::Rav1e => {',a)
s=s[:a]+'''        Backend::Aom => {
            let mut k=aom_encode::key_frame::KeyFrameConfig::allintra_speed0(w,h,cfg.bit_depth,cfg.chroma==Chroma::Mono,sx,sy,cfg.quantizer as i32);
            k.cpu_used=cfg.speed as i32;k.enable_restoration=true;k.sb_size_128=cfg.sb128;
            let s=samples();
            aom_encode::key_frame::encode_key_frame(aom_encode::key_frame::KeyFramePlanes {y:&s[0],u:&s[1],v:&s[2]},&k).map_err(|e|e.to_string())
        }
'''+s[b:]
a=s.index('        Backend::Rav1e => {',s.index('pub fn encode'));b=s.index('/// Independent libaom decode',a)
s=s[:a]+'''        Backend::Rav1e => {
            if cfg.bit_depth==8 {rav_encode::<u8>(cfg,[y,u,v])}else{rav_encode::<u16>(cfg,[y,u,v])}
        }
    }
}
fn rav_encode<T:zenrav1e::prelude::Pixel>(cfg:Config,planes:[&[u8];3])->Result<Vec<u8>,String> {
    use zenrav1e::prelude::*;
    let chroma_sampling=match cfg.chroma {Chroma::Cs420=>ChromaSampling::Cs420,Chroma::Cs422=>ChromaSampling::Cs422,Chroma::Cs444=>ChromaSampling::Cs444,Chroma::Mono=>ChromaSampling::Cs400};
    let e=EncoderConfig {width:cfg.width as usize,height:cfg.height as usize,bit_depth:cfg.bit_depth as usize,
        chroma_sampling,pixel_range:PixelRange::Limited,still_picture:true,quantizer:cfg.quantizer as usize,
        min_quantizer:cfg.quantizer as u8,speed_settings:SpeedSettings::from_preset(cfg.speed as u8),..Default::default()};
    let mut ctx:Context<T>=zenrav1e::Config::new().with_encoder_config(e).with_threads(cfg.threads as usize).new_context().map_err(|e|format!("{e:?}"))?;
    let mut f=ctx.new_frame();let bps=if cfg.bit_depth==8 {1}else{2};let (sx,_)=cfg.chroma.shifts();
    for (i,p) in planes.into_iter().enumerate() {if !p.is_empty() {
        f.planes[i].copy_from_raw_u8(p,if i==0 {cfg.width as usize*bps}else{(cfg.width as usize).div_ceil(1<<sx)*bps},bps);
    }}
    ctx.send_frame(f).map_err(|e|e.to_string())?;ctx.flush();let mut bytes=Vec::new();
    loop {match ctx.receive_packet(){Ok(p)=>bytes.extend(p.data),Err(EncoderStatus::Encoded)=>continue,
        Err(EncoderStatus::LimitReached)=>break,Err(e)=>return Err(e.to_string())}}
    if bytes.is_empty(){Err("zenrav1e returned no frame".into())}else{Ok(bytes)}
}
'''+s[b:]
# Add fields to tests' existing cfg literals.
s=s.replace('threads: 1,','threads: 1, bit_depth:8,chroma:Chroma::Cs420,tune:None,scm:None,sb128:false,')
p.write_text(s)
