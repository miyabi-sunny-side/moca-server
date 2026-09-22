//! Isolated resident-process double. No VOICEPEAK library or audio device is used.
use std::{
    fs,
    io::{self, Read, Write},
    process, thread,
    time::Duration,
};

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    output.write_all(b"R")?;
    output.flush()?;
    loop {
        let mut count = [0; 4];
        match input.read_exact(&mut count) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        let mut args = Vec::new();
        for _ in 0..u32::from_le_bytes(count) {
            let mut size = [0; 4];
            input.read_exact(&mut size)?;
            let mut bytes = vec![0; u32::from_le_bytes(size) as usize];
            input.read_exact(&mut bytes)?;
            args.push(String::from_utf8(bytes).unwrap());
        }
        let text = &args[args.iter().position(|a| a == "-s").unwrap() + 1];
        let file = &args[args.iter().position(|a| a == "-o").unwrap() + 1];
        match text.as_str() {
            "stall" => thread::sleep(Duration::from_secs(30)),
            "crash" => process::exit(3),
            _ => {}
        }
        if text == "bad" {
            fs::write(file, b"incomplete")?;
        } else {
            // Independent fixed WAV header, matching the old wave module's fixture:
            // PCM, mono, 48kHz, 16-bit, four data bytes (RIFF size 40).
            let mut wav = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\xbb\0\0\0\x77\x01\0\x02\0\x10\0data\x04\0\0\0".to_vec();
            wav.extend([text.as_bytes()[0]; 4]);
            fs::write(file, wav)?;
        }
        output.write_all(b"D")?;
        output.flush()?;
    }
}
