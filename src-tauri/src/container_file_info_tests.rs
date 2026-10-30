use super::*;
use std::io::Cursor;
fn inspect(b: Vec<u8>) -> Result<LocalFileInfo, InfoError> {
    super::super::read_file_info(&mut Cursor::new(&b), b.len() as u64)
}
fn chunk(id: &[u8; 4], data: &[u8], big: bool) -> Vec<u8> {
    let mut b = id.to_vec();
    b.extend(if big {
        (data.len() as u32).to_be_bytes()
    } else {
        (data.len() as u32).to_le_bytes()
    });
    b.extend(data);
    if data.len() % 2 == 1 {
        b.push(0);
    }
    b
}
fn wave(rate: u32, depth: u16, ch: u16, float: bool) -> Vec<u8> {
    let align = ch * (depth / 8);
    let mut fmt = Vec::new();
    fmt.extend((if float { 3u16 } else { 1 }).to_le_bytes());
    fmt.extend(ch.to_le_bytes());
    fmt.extend(rate.to_le_bytes());
    fmt.extend((rate * u32::from(align)).to_le_bytes());
    fmt.extend(align.to_le_bytes());
    fmt.extend(depth.to_le_bytes());
    let mut b = b"WAVE".to_vec();
    b.extend(chunk(b"fmt ", &fmt, false));
    b.extend(chunk(b"data", &vec![0; usize::from(align) * 10], false));
    let mut out = b"RIFF".to_vec();
    out.extend((b.len() as u32).to_le_bytes());
    out.extend(b);
    out
}
fn aiff(aifc: bool, codec: &[u8; 4]) -> Vec<u8> {
    // 44100 exactly in IEEE 80-bit extended precision: 400e ac44 000000000000.
    let mut comm = vec![
        0, 2, 0, 0, 0, 10, 0, 16, 0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0,
    ];
    if aifc {
        comm.extend(codec);
        comm.extend([0, 0]);
    }
    let mut b = if aifc {
        b"AIFC".to_vec()
    } else {
        b"AIFF".to_vec()
    };
    b.extend(chunk(b"COMM", &comm, true));
    b.extend(chunk(b"SSND", &[0; 48], true));
    let mut out = b"FORM".to_vec();
    out.extend((b.len() as u32).to_be_bytes());
    out.extend(b);
    out
}
fn ogg_page(packet: &[u8], serial: u32, seq: u32, flags: u8) -> Vec<u8> {
    let mut b = b"OggS\0".to_vec();
    b.push(flags);
    b.extend(0u64.to_le_bytes());
    b.extend(serial.to_le_bytes());
    b.extend(seq.to_le_bytes());
    b.extend([0; 4]);
    let mut laces = vec![255; packet.len() / 255];
    laces.push((packet.len() % 255) as u8);
    b.push(laces.len() as u8);
    b.extend(laces);
    b.extend(packet);
    b
}
fn vorbis() -> Vec<u8> {
    let mut b = b"\x01vorbis\0\0\0\0\x02".to_vec();
    b.extend(44100u32.to_le_bytes());
    b.extend([0; 12]);
    b.extend([0xb8, 1]);
    b
}
fn opus() -> Vec<u8> {
    let mut b = b"OpusHead\x01\x02\x38\x01".to_vec();
    b.extend(12345u32.to_le_bytes());
    b.extend([0, 0, 0]);
    b
}
fn wv(hybrid: bool) -> Vec<u8> {
    let mut b = b"wvpk".to_vec();
    b.extend(24u32.to_le_bytes());
    b.extend([2, 4, 0, 0]);
    b.extend(44100u32.to_le_bytes());
    b.extend(0u32.to_le_bytes());
    b.extend(44100u32.to_le_bytes());
    b.extend(((9u32 << 23) | 0x1801 | if hybrid { 8 } else { 0 }).to_le_bytes());
    b.extend([0; 4]);
    b.resize(160, 0);
    b
}
#[test]
fn pcm_matrix_and_float_are_container_properties_not_master_claims() {
    for (rate, depth, ch, float) in [
        (44100, 16, 2, false),
        (48000, 24, 2, false),
        (96000, 16, 1, false),
        (96000, 24, 2, false),
        (48000, 32, 2, true),
        (96000, 64, 2, true),
    ] {
        let r = inspect(wave(rate, depth, ch, float)).unwrap();
        assert_eq!(
            (r.sample_rate, r.bit_depth, r.channels),
            (Some(rate), Some(depth as u8), ch as u8)
        );
        assert_eq!(r.compression, "uncompressed");
        assert_eq!(r.sample_type, if float { "float" } else { "integer" });
        assert_eq!(r.duration_seconds, Some(10.0 / f64::from(rate)));
    }
    for (aifc, codec) in [(false, *b"NONE"), (true, *b"NONE"), (true, *b"sowt")] {
        let r = inspect(aiff(aifc, &codec)).unwrap();
        assert_eq!(
            (r.sample_rate, r.bit_depth, r.channels),
            (Some(44100), Some(16), 2)
        );
    }
    assert_eq!(
        inspect(aiff(true, b"MAC3")).unwrap_err(),
        InfoError::UnsupportedFormat
    );
    let mut b = wave(44100, 16, 2, false);
    b[20] = 0x55;
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
}
#[test]
fn wav_extensible_guid_valid_bits_and_alignment_are_verified() {
    let mut b = wave(96000, 32, 2, false);
    let mut fmt = b[20..36].to_vec();
    fmt[..2].copy_from_slice(&0xfffeu16.to_le_bytes());
    fmt.extend([
        22, 0, 24, 0, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71,
    ]);
    let mut body = b"WAVE".to_vec();
    body.extend(chunk(b"fmt ", &fmt, false));
    body.extend(chunk(b"data", &[0; 80], false));
    b = b"RIFF".to_vec();
    b.extend((body.len() as u32).to_le_bytes());
    b.extend(body);
    assert_eq!(inspect(b.clone()).unwrap().bit_depth, Some(24));
    b[47] ^= 1;
    assert!(inspect(b).is_err());
}
#[test]
fn adts_three_headers_match_core_clock_not_effective_he_aac_rate() {
    // MPEG4 AAC-LC, 44.1 kHz stereo, 9-byte frame, one raw_data_block.
    let frame = [0xff, 0xf1, 0x50, 0x80, 0x01, 0x3f, 0xfc, 0, 0];
    let b = frame.repeat(3);
    let r = inspect(b.clone()).unwrap();
    assert_eq!(
        (r.codec, r.sample_rate, r.channels, r.bit_depth),
        ("AAC", Some(44100), 2, None)
    );
    assert_eq!(r.sample_rate_basis, "aac_core_header");
    assert_eq!(r.duration_seconds, None);
    let mut bad = b.clone();
    bad[11] ^= 4;
    assert!(inspect(bad).is_err());
    assert!(inspect(b[..26].to_vec()).is_err());
    let mut pce = b;
    for i in [2, 11, 20] {
        pce[i] &= !1;
        pce[i + 1] &= 0x3f;
    }
    assert_eq!(inspect(pce).unwrap_err(), InfoError::UnsupportedFormat);
}
#[test]
fn ogg_identification_distinguishes_three_codecs_and_opus_clocks() {
    let r = inspect(ogg_page(&vorbis(), 7, 0, 6)).unwrap();
    assert_eq!(
        (r.format, r.codec, r.sample_rate),
        ("Ogg", "Vorbis", Some(44100))
    );
    let r = inspect(ogg_page(&opus(), 8, 0, 6)).unwrap();
    assert_eq!(
        (r.sample_rate, r.original_sample_rate, r.sample_rate_basis),
        (Some(48000), Some(12345), "opus_decode_clock")
    );
    assert_eq!(r.bit_depth, None);
    let mut p = b"\x7fFLAC\x01\0\0\x01".to_vec();
    p.extend(super::super::tests::fixture(96000, 24, 2, 96000));
    let r = inspect(ogg_page(&p, 9, 0, 6)).unwrap();
    assert_eq!(
        (r.codec, r.bit_depth, r.duration_seconds),
        ("FLAC", Some(24), Some(1.0))
    );
}
#[test]
fn ogg_multiplex_chaining_truncation_and_page_budget_are_not_silently_selected() {
    let mut b = ogg_page(&vorbis(), 7, 0, 2);
    b.extend(ogg_page(&[0], 8, 0, 6));
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
    let mut b = ogg_page(&vorbis(), 7, 0, 6);
    b.extend(ogg_page(&vorbis(), 8, 0, 6));
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
    let mut b = ogg_page(&opus(), 7, 0, 2);
    for i in 1..1025 {
        b.extend(ogg_page(&[0], 7, i, if i == 1024 { 4 } else { 0 }));
    }
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
    let b = ogg_page(&vorbis(), 1, 0, 6);
    for n in 0..b.len() {
        assert!(inspect(b[..n].to_vec()).is_err(), "{n}");
    }
}
#[test]
fn wavpack_lossless_and_hybrid_never_auto_read_correction_file() {
    for hybrid in [false, true] {
        let r = inspect(wv(hybrid)).unwrap();
        assert_eq!(
            (r.sample_rate, r.bit_depth, r.channels),
            (Some(44100), Some(16), 2)
        );
        assert_eq!(r.compression, if hybrid { "hybrid" } else { "lossless" });
    }
    let mut b = wv(false);
    b[27] |= 0x80;
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
    let mut b = wv(false);
    b[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(inspect(b).is_err());
}
#[test]
fn musepack_sv7_and_sv8_header_only_no_audio_scan() {
    let mut b = b"MP+\x07".to_vec();
    b.extend(10u32.to_le_bytes());
    b.extend([0, 0, 0x50, 0]);
    b.resize(160, 0);
    let r = inspect(b).unwrap();
    assert_eq!(
        (r.format, r.codec, r.sample_rate, r.bit_depth),
        ("Musepack SV7", "Musepack", Some(44100), None)
    );
    let b = b"MPCKSH\x0e\x00\x00\x00\x01\x08\x82\xd8\x44\x00\x1f\x10AP\x04\x00".to_vec();
    let r = inspect(b.clone()).unwrap();
    assert_eq!(
        (r.format, r.sample_rate, r.channels, r.duration_seconds),
        ("Musepack SV8", Some(44100), 2, Some(1.0))
    );
    for n in 0..b.len() {
        assert!(inspect(b[..n].to_vec()).is_err(), "{n}");
    }
}
#[test]
fn structural_bombs_and_chunk_parent_bounds_refused_before_payload_reads() {
    let b = wave(44100, 16, 2, false);
    for n in 0..b.len() {
        assert!(inspect(b[..n].to_vec()).is_err(), "{n}");
    }
    let mut b = wave(44100, 16, 2, false);
    b[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(inspect(b).is_err());
    let mut b = wave(44100, 16, 2, false);
    b[16..20].copy_from_slice(&1024u32.to_le_bytes());
    b.resize(2048, 0);
    b[4..8].copy_from_slice(&2040u32.to_le_bytes());
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
    let mut body = b"WAVE".to_vec();
    for _ in 0..257 {
        body.extend(chunk(b"JUNK", &[], false));
    }
    let mut b = b"RIFF".to_vec();
    b.extend((body.len() as u32).to_le_bytes());
    b.extend(body);
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
}
#[test]
fn io_limits_cover_partial_interrupted_seeks_and_never_read_pcm_payload() {
    struct Audit {
        c: Cursor<Vec<u8>>,
        calls: usize,
        bytes: usize,
        seeks: usize,
    }
    impl Read for Audit {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls % 5 == 0 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.c.position() >= 44 && !b.is_empty() {
                panic!("PCM payload read");
            }
            let n = b.len().min(1);
            let got = self.c.read(&mut b[..n])?;
            self.bytes += got;
            Ok(got)
        }
    }
    impl Seek for Audit {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.seeks += 1;
            self.c.seek(p)
        }
    }
    let b = wave(44100, 16, 2, false);
    let len = b.len() as u64;
    let mut a = Audit {
        c: Cursor::new(b),
        calls: 0,
        bytes: 0,
        seeks: 0,
    };
    let r = super::super::read_file_info(&mut a, len).unwrap();
    assert_eq!(r.channels, 2);
    assert!(a.bytes < 256 && a.calls < 512 && a.seeks < 128);
    struct Interrupted;
    impl Read for Interrupted {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::Interrupted.into())
        }
    }
    impl Seek for Interrupted {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            match p {
                SeekFrom::Start(n) => Ok(n),
                _ => Ok(0),
            }
        }
    }
    let mut raw = Interrupted;
    let mut b = Budget::new(&mut raw, 100, 64, 80, 16);
    let mut data = [0; 8];
    assert!(b.read_exact(&mut data).is_err());
    assert_eq!(b.reads, 80);
    assert!(b.seek(SeekFrom::Start(101)).is_err());
    assert_eq!(b.failure, Some(InfoError::CorruptHeader));
}
#[test]
fn generated_encoder_fixtures_if_explicitly_provided() {
    // Optional independent encoder corpus, no implicit music/profile access.
    let Some(dir) = std::env::var_os("SONANTE_FORMAT_FIXTURES") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    for (name, codec, rate, ch) in [
        ("pcm16.wav", "PCM", 44100, 2),
        ("pcm24_48.wav", "PCM", 48000, 2),
        ("pcm16_96.wav", "PCM", 96000, 1),
        ("pcm24_96.wav", "PCM", 96000, 2),
        ("float32.wav", "PCM float", 48000, 2),
        ("integer.aiff", "PCM", 44100, 2),
        ("swapped.aifc", "PCM", 48000, 1),
        ("aac.adts", "AAC", 44100, 2),
        ("aac.m4a", "AAC", 44100, 1),
        ("alac.m4a", "ALAC", 96000, 2),
        ("vorbis.ogg", "Vorbis", 44100, 2),
        ("opus.ogg", "Opus", 48000, 2),
        ("flac.ogg", "FLAC", 96000, 2),
        ("integer.wv", "WavPack", 96000, 2),
        ("hybrid.wv", "WavPack", 44100, 2),
        ("float.wv", "WavPack", 48000, 2),
    ] {
        let path = dir.join(name);
        let mut f = std::fs::File::open(&path).unwrap();
        let len = f.metadata().unwrap().len();
        let r =
            super::super::read_file_info(&mut f, len).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        if name == "hybrid.wv" {
            assert_eq!(r.compression, "hybrid");
        }
        if name == "float.wv" {
            assert_eq!((r.sample_type, r.bit_depth), ("float", Some(32)));
        }
        assert_eq!(
            (r.codec, r.sample_rate, r.channels),
            (codec, Some(rate), ch),
            "{name}"
        );
    }
}
fn atom(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut b = ((data.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend(id);
    b.extend(data);
    b
}
fn mp4_fixture(codec: &[u8; 4], aot: u8, tracks: usize, stts_count: u32) -> Vec<u8> {
    let config = if codec == b"alac" {
        let mut c = vec![0; 28];
        c[4..8].copy_from_slice(&4096u32.to_be_bytes());
        c[9] = 24;
        c[10..14].copy_from_slice(&[40, 10, 14, 2]);
        c[24..28].copy_from_slice(&96000u32.to_be_bytes());
        atom(b"alac", &c)
    } else {
        let mut asc = vec![5, 2, (aot << 3) | 2, 0x10];
        let mut decoder = vec![0x40, 0x15];
        decoder.extend([0; 11]);
        decoder.append(&mut asc);
        let mut d = vec![4, decoder.len() as u8];
        d.extend(decoder);
        let mut es = vec![0, 1, 0];
        es.extend(d);
        let mut c = vec![0; 4];
        c.extend([3, es.len() as u8]);
        c.extend(es);
        atom(b"esds", &c)
    };
    let mut entry = vec![0; 28];
    entry[7] = 1;
    entry[16..18].copy_from_slice(&2u16.to_be_bytes());
    entry[18..20].copy_from_slice(&16u16.to_be_bytes());
    entry[24..28].copy_from_slice(&(44100u32 << 16).to_be_bytes());
    entry.extend(config);
    let mut stsd = vec![0; 4];
    stsd.extend(1u32.to_be_bytes());
    stsd.extend(atom(codec, &entry));
    let mut table = atom(b"stsd", &stsd);
    let mut stts = vec![0; 4];
    stts.extend(stts_count.to_be_bytes());
    if stts_count == 1 {
        stts.extend(10u32.to_be_bytes());
        stts.extend(1024u32.to_be_bytes());
    }
    table.extend(atom(b"stts", &stts));
    let mut hdlr = vec![0; 8];
    hdlr.extend(b"soun");
    hdlr.extend([0; 12]);
    let mut mdhd = vec![0; 12];
    mdhd.extend(44100u32.to_be_bytes());
    mdhd.extend(44100u32.to_be_bytes());
    mdhd.extend([0; 4]);
    let mut mdia = atom(b"mdhd", &mdhd);
    mdia.extend(atom(b"hdlr", &hdlr));
    mdia.extend(atom(b"minf", &atom(b"stbl", &table)));
    let track = atom(b"trak", &atom(b"mdia", &mdia));
    let mut moov = Vec::new();
    for _ in 0..tracks {
        moov.extend(&track);
    }
    let mut out = atom(b"ftyp", b"M4A \0\0\0\0isom");
    out.extend(atom(b"mdat", &[0; 16]));
    out.extend(atom(b"moov", &moov));
    out
}
#[test]
fn mp4_decoder_configs_prove_aac_vs_alac_and_reject_ambiguous_variants() {
    let r = inspect(mp4_fixture(b"mp4a", 2, 1, 1)).unwrap();
    assert_eq!(
        (r.codec, r.sample_rate, r.channels, r.bit_depth),
        ("AAC", Some(44100), 2, None)
    );
    assert_eq!(r.duration_seconds, Some(1.0));
    let r = inspect(mp4_fixture(b"alac", 2, 1, 1)).unwrap();
    assert_eq!(
        (r.codec, r.sample_rate, r.channels, r.bit_depth),
        ("ALAC", Some(96000), 2, Some(24))
    );
    for b in [
        mp4_fixture(b"mp4a", 5, 1, 1),
        mp4_fixture(b"mp4a", 2, 2, 1),
        mp4_fixture(b"enca", 2, 1, 1),
        mp4_fixture(b"mp4a", 2, 1, u32::MAX),
    ] {
        assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
    }
    let mut b = mp4_fixture(b"mp4a", 2, 1, 1);
    let i = b.windows(4).position(|v| v == b"stsd").unwrap();
    b[i + 8..i + 12].copy_from_slice(&2u32.to_be_bytes());
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
}
#[test]
fn mp4_truncation_parent_bounds_huge_stsd_and_descriptor_overflow_are_bounded() {
    let b = mp4_fixture(b"mp4a", 2, 1, 1);
    for n in 0..b.len() {
        assert!(inspect(b[..n].to_vec()).is_err(), "{n}");
    }
    let mut b = mp4_fixture(b"mp4a", 2, 1, 1);
    let i = b.windows(4).position(|v| v == b"esds").unwrap();
    b[i + 9..i + 13].copy_from_slice(&[255; 4]);
    assert!(inspect(b).is_err());
    let mut b = mp4_fixture(b"mp4a", 2, 1, 1);
    let i = b.windows(4).position(|v| v == b"stsd").unwrap();
    b[i - 4..i].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(inspect(b).is_err());
    let mut b = mp4_fixture(b"mp4a", 2, 1, 1);
    b[..4].copy_from_slice(&1u32.to_be_bytes());
    assert_eq!(inspect(b).unwrap_err(), InfoError::UnsupportedFormat);
}
#[test]
fn library_parses_only_frozen_validated_metadata_even_if_underlying_header_changes() {
    struct Swap {
        c: Cursor<Vec<u8>>,
        fmt_reads: usize,
    }
    impl Read for Swap {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let pos = self.c.position();
            let n = self.c.read(b)?;
            if pos == 20 {
                self.fmt_reads += 1;
                self.c.get_mut()[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
            }
            Ok(n)
        }
    }
    impl Seek for Swap {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.c.seek(p)
        }
    }
    let b = wave(44100, 16, 2, false);
    let len = b.len() as u64;
    let mut r = Swap {
        c: Cursor::new(b),
        fmt_reads: 0,
    };
    let info = super::super::read_file_info(&mut r, len).unwrap();
    assert_eq!(info.bit_depth, Some(16));
    assert_eq!(r.fmt_reads, 1);
}
#[test]
fn giant_iff_tag_and_pcm_payload_are_skipped_not_copied_into_library_snapshot() {
    let fmt = wave(44100, 16, 2, false)[20..36].to_vec();
    let skip = 256 * 1024 * 1024u64;
    let mut prefix = b"WAVE".to_vec();
    prefix.extend(chunk(b"fmt ", &fmt, false));
    prefix.extend(b"ID3 ");
    prefix.extend((skip as u32).to_le_bytes());
    let data_start = 8 + prefix.len() as u64 + skip;
    let tail = chunk(b"data", &[0; 40], false);
    let len = data_start + tail.len() as u64;
    let mut head = b"RIFF".to_vec();
    head.extend(((len - 8) as u32).to_le_bytes());
    head.extend(prefix);
    struct Sparse {
        head: Vec<u8>,
        tail: Vec<u8>,
        tail_at: u64,
        pos: u64,
        read: usize,
    }
    impl Read for Sparse {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let (data, i) = if self.pos < self.head.len() as u64 {
                (&self.head, self.pos as usize)
            } else if self.pos >= self.tail_at && self.pos < self.tail_at + 8 {
                (&self.tail, (self.pos - self.tail_at) as usize)
            } else {
                panic!("tag or audio payload read at {}", self.pos)
            };
            let n = b.len().min(data.len() - i);
            b[..n].copy_from_slice(&data[i..i + n]);
            self.pos += n as u64;
            self.read += n;
            Ok(n)
        }
    }
    impl Seek for Sparse {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            match p {
                SeekFrom::Start(n) => {
                    self.pos = n;
                    Ok(n)
                }
                _ => panic!("absolute seeks only"),
            }
        }
    }
    let mut r = Sparse {
        head,
        tail,
        tail_at: data_start,
        pos: 0,
        read: 0,
    };
    let info = super::super::read_file_info(&mut r, len).unwrap();
    assert_eq!(info.duration_seconds, Some(10.0 / 44100.0));
    assert!(r.read < 128);
}

#[test]
fn mpc7_zero_gapless_samples_and_size_overflow_never_enter_panicking_library_math() {
    let mut b = b"MP+\x07".to_vec();
    b.extend(1u32.to_le_bytes());
    b.extend([0, 0, 0x50, 0]);
    b.resize(160, 0);
    b[20..24].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    assert_eq!(inspect(b.clone()).unwrap_err(), InfoError::CorruptHeader);
    b[20..24].copy_from_slice(&0u32.to_le_bytes());
    let mut reader = Cursor::new(b);
    assert_eq!(
        super::super::read_file_info(&mut reader, u64::MAX).unwrap_err(),
        InfoError::UnsupportedFormat
    );
}

#[test]
fn adts_initial_header_interruptions_stay_inside_family_call_budget() {
    struct Interrupted {
        reads: usize,
    }
    impl Read for Interrupted {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            if self.reads == 1 {
                b[..4].copy_from_slice(&[0xff, 0xf1, 0x50, 0x80]);
                Ok(4)
            } else {
                Err(io::ErrorKind::Interrupted.into())
            }
        }
    }
    impl Seek for Interrupted {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            if let SeekFrom::Start(n) = p {
                Ok(n)
            } else {
                panic!("absolute seeks")
            }
        }
    }
    let mut r = Interrupted { reads: 0 };
    assert_eq!(
        super::super::read_file_info(&mut r, 100).unwrap_err(),
        InfoError::UnsupportedFormat
    );
    assert!(r.reads <= 2048);
}

#[test]
fn ogg_giant_comment_payload_is_skipped_without_reading_or_allocating_it() {
    let first = ogg_page(&opus(), 7, 0, 2);
    let mut b = first.clone();
    let comment_start = b.len() + 27 + 255;
    b.extend(ogg_page(&vec![0; 65024], 7, 1, 0));
    let comment_end = b.len();
    b.extend(ogg_page(&[0], 7, 2, 4));
    struct Audit {
        c: Cursor<Vec<u8>>,
        start: u64,
        end: u64,
        bytes: usize,
    }
    impl Read for Audit {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let p = self.c.position();
            assert!(p < self.start || p >= self.end, "comment payload read");
            let n = self.c.read(b)?;
            self.bytes += n;
            Ok(n)
        }
    }
    impl Seek for Audit {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.c.seek(p)
        }
    }
    let len = b.len() as u64;
    let mut r = Audit {
        c: Cursor::new(b),
        start: comment_start as u64,
        end: comment_end as u64,
        bytes: 0,
    };
    let info = super::super::read_file_info(&mut r, len).unwrap();
    assert_eq!(info.codec, "Opus");
    assert!(r.bytes < 512);
}
