//! Diagnostic: compare a file's reported n_frames vs its decoded frame count.
//! Usage: cargo run -p flint-music --example probe_file -- <path> [<path>...]

use std::fs::File;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

fn main() {
    for path in std::env::args().skip(1) {
        let file = File::open(&path).expect("open");
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = std::path::Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
        {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .expect("probe");
        let mut format = probed.format;
        let track = format.default_track().expect("track");
        let track_id = track.id;
        let params = track.codec_params.clone();
        let reported = params.n_frames;
        let delay = params.delay;
        let padding = params.padding;

        let mut decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .expect("decoder");
        let mut decoded: u64 = 0;
        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break
                }
                Err(symphonia::core::errors::Error::ResetRequired) => break,
                Err(e) => {
                    eprintln!("{path}: {e}");
                    break;
                }
            };
            if packet.track_id() != track_id {
                continue;
            }
            match decoder.decode(&packet) {
                Ok(buf) => decoded += buf.frames() as u64,
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(e) => {
                    eprintln!("{path}: {e}");
                    break;
                }
            }
        }
        println!(
            "{path}: n_frames={reported:?} decoded={decoded} delay={delay:?} padding={padding:?}"
        );
    }
}
