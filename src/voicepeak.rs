use crate::wav::extract_pcm;
use std::{path::Path, process::Stdio, time::Duration};

#[derive(Default)]
pub struct Voicepeak {
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    enabled: Option<bool>,
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    process: Option<resident::Process>,
}

impl Voicepeak {
    pub async fn synthesize(
        &mut self,
        executable: &str,
        args: &[String],
        output: &Path,
    ) -> Result<Vec<u8>, String> {
        tokio::time::timeout(Duration::from_secs(60), self.run(executable, args, output))
            .await
            .map_err(|_| "voicepeak timed out after 60 seconds".to_string())?
    }

    async fn run(
        &mut self,
        executable: &str,
        args: &[String],
        output: &Path,
    ) -> Result<Vec<u8>, String> {
        #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
        if *self
            .enabled
            .get_or_insert_with(|| resident::supported(executable))
        {
            // Keep ownership in this future until the complete WAV is validated.
            // Cancellation, timeout or any failure drops and kills only our child.
            let mut process = match self.process.take() {
                Some(process) => process,
                None => match resident::Process::start(executable).await {
                    Ok(process) => process,
                    Err(error) => {
                        self.enabled = Some(false);
                        tracing::warn!(%error, "VOICEPEAK resident unavailable; retrying with CLI");
                        return Err(error);
                    }
                },
            };
            process.request(args).await?;
            let bytes = tokio::fs::read(output)
                .await
                .map_err(|e| format!("read output: {e}"))?;
            let pcm = resident::complete_pcm(&bytes)?;
            self.process = Some(process);
            return Ok(pcm);
        }

        let status = tokio::process::Command::new(executable)
            .args(&args[1..])
            .kill_on_drop(true)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .await
            .map_err(|e| format!("spawn: {e}"))?;
        if !status.success() {
            return Err(format!("exited with {status}"));
        }
        let bytes = tokio::fs::read(output)
            .await
            .map_err(|e| format!("read output: {e}"))?;
        extract_pcm(&bytes)
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
mod resident {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::{Child, ChildStdin, ChildStdout};

    const BUILD_ID: &[u8; 20] = &[
        0x65, 0xa2, 0xef, 0xfb, 0xee, 0x1d, 0x5d, 0x90, 0x8a, 0x73, 0x5d, 0x1b, 0x66, 0xb4, 0xaa,
        0xad, 0x20, 0x71, 0xbb, 0xc4,
    ];

    fn supported_header(header: &[u8]) -> bool {
        header.starts_with(b"\x7fELF\x02\x01\x01") && header.get(0x30c..0x320) == Some(BUILD_ID)
    }

    pub(super) fn supported(executable: &str) -> bool {
        if std::env::var("MOCA_VOICEPEAK_RESIDENT").as_deref() == Ok("0") {
            return false;
        }
        let path = if executable.contains('/') {
            Some(std::path::PathBuf::from(executable))
        } else {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| p.join(executable))
                    .find(|p| {
                        p.metadata()
                            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    })
            })
        };
        let mut header = [0; 0x320];
        let supported = path
            .and_then(|p| std::fs::File::open(p).ok())
            .is_some_and(|mut f| f.read_exact(&mut header).is_ok() && supported_header(&header));
        if !supported {
            tracing::info!("VOICEPEAK build is not supported by resident mode; using CLI");
        }
        supported
    }

    fn encode_args(args: &[String]) -> Result<Vec<u8>, String> {
        if args.is_empty()
            || args.len() > 32
            || args.iter().any(|a| a.contains('\0'))
            || args.iter().map(String::len).sum::<usize>() > 1024 * 1024
        {
            return Err("invalid resident voicepeak arguments".into());
        }
        let mut bytes = (args.len() as u32).to_le_bytes().to_vec();
        for arg in args {
            bytes.extend_from_slice(&(arg.len() as u32).to_le_bytes());
            bytes.extend_from_slice(arg.as_bytes());
        }
        Ok(bytes)
    }

    pub(super) fn complete_pcm(bytes: &[u8]) -> Result<Vec<u8>, String> {
        if bytes.len() < 12
            || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8 != bytes.len()
        {
            return Err("incomplete resident WAV".into());
        }
        let mut pos = 12;
        while pos < bytes.len() {
            let header = bytes
                .get(pos..pos + 8)
                .ok_or("incomplete WAV chunk header")?;
            let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            pos += 8 + size + (size & 1);
            if pos > bytes.len() {
                return Err("incomplete WAV chunk".into());
            }
        }
        let pcm = extract_pcm(bytes)?;
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            return Err("empty or incomplete resident PCM".into());
        }
        Ok(pcm)
    }

    pub(super) struct Process {
        child: Child,
        input: ChildStdin,
        output: ChildStdout,
        _library: tempfile::NamedTempFile,
    }

    impl Process {
        pub(super) async fn start(executable: &str) -> Result<Self, String> {
            let mut library = tempfile::Builder::new()
                .prefix("moca-resident-")
                .suffix(".so")
                .tempfile()
                .map_err(|e| format!("resident library: {e}"))?;
            library
                .write_all(include_bytes!(concat!(
                    env!("OUT_DIR"),
                    "/voicepeak_resident.so"
                )))
                .map_err(|e| format!("resident library: {e}"))?;
            let mut preload = library.path().as_os_str().to_os_string();
            if let Some(existing) = std::env::var_os("LD_PRELOAD") {
                preload.push(":");
                preload.push(existing);
            }
            let mut child = tokio::process::Command::new(executable)
                .arg("--help")
                .env("LD_PRELOAD", preload)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("resident spawn: {e}"))?;
            let input = child.stdin.take().unwrap();
            let output = child.stdout.take().unwrap();
            let mut process = Self {
                child,
                input,
                output,
                _library: library,
            };
            process.expect(b'R').await?;
            tracing::info!(pid = process.child.id(), "VOICEPEAK resident started");
            Ok(process)
        }

        async fn expect(&mut self, expected: u8) -> Result<(), String> {
            let value = self
                .output
                .read_u8()
                .await
                .map_err(|e| format!("resident response: {e}"))?;
            if value != expected {
                return Err("invalid resident response".into());
            }
            Ok(())
        }

        pub(super) async fn request(&mut self, args: &[String]) -> Result<(), String> {
            let wire = encode_args(args)?;
            self.input
                .write_all(&wire)
                .await
                .map_err(|e| format!("resident request: {e}"))?;
            self.expect(b'D').await
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn requires_the_verified_elf_build() {
            let mut header = vec![0; 0x320];
            header[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
            header[0x30c..0x320].copy_from_slice(BUILD_ID);
            assert!(supported_header(&header));
            header[0x31f] ^= 1;
            assert!(!supported_header(&header));
            assert!(!supported_header(b"#!/bin/sh"));
        }

        #[test]
        fn frames_utf8_arguments_and_rejects_invalid_requests() {
            let args = vec!["voicepeak".into(), "雨\n声".into(), "/tmp/a wav".into()];
            let wire = encode_args(&args).unwrap();
            let mut expected = 3u32.to_le_bytes().to_vec();
            expected.extend(9u32.to_le_bytes());
            expected.extend(b"voicepeak");
            expected.extend(7u32.to_le_bytes());
            expected.extend("雨\n声".as_bytes());
            expected.extend(10u32.to_le_bytes());
            expected.extend(b"/tmp/a wav");
            assert_eq!(wire, expected);
            assert!(encode_args(&[]).is_err());
            assert!(encode_args(&vec![String::new(); 33]).is_err());
            assert!(encode_args(&["a\0b".into()]).is_err());
            assert!(encode_args(&["a".repeat(1024 * 1024 + 1)]).is_err());
        }

        #[test]
        fn rejects_unfinished_or_empty_wavs() {
            let mut wav = crate::wav::wav_header(&crate::wav::MOCA_FORMAT);
            wav.extend([1, 2, 3, 4]);
            wav[4..8].copy_from_slice(&40u32.to_le_bytes());
            wav[40..44].copy_from_slice(&4u32.to_le_bytes());
            assert_eq!(complete_pcm(&wav).unwrap(), [1, 2, 3, 4]);
            assert!(complete_pcm(&wav[..47]).is_err());
            wav[40..44].copy_from_slice(&100u32.to_le_bytes());
            assert!(complete_pcm(&wav).is_err());
            wav.truncate(44);
            wav[4..8].copy_from_slice(&36u32.to_le_bytes());
            wav[40..44].copy_from_slice(&0u32.to_le_bytes());
            assert!(complete_pcm(&wav).is_err());
        }

        async fn fake_process() -> Process {
            let script = r#"
import os, struct, sys, time, wave
src, dst = sys.stdin.buffer, sys.stdout.buffer
dst.write(b'R'); dst.flush()
while True:
    count = src.read(4)
    if not count: break
    args = [src.read(struct.unpack('<I', src.read(4))[0]).decode() for _ in range(struct.unpack('<I', count)[0])]
    text, out = args[args.index('-s') + 1], args[args.index('-o') + 1]
    if text == 'stall': time.sleep(30)
    if text == 'crash': os._exit(3)
    if text == 'bad': open(out, 'wb').write(b'incomplete')
    else:
        with wave.open(out, 'wb') as wav:
            wav.setparams((1, 2, 48000, 0, 'NONE', 'none'))
            wav.writeframes(text.encode()[:1] * 4)
    dst.write(b'D'); dst.flush()
"#;
            let mut child = tokio::process::Command::new("python3")
                .args(["-u", "-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let input = child.stdin.take().unwrap();
            let output = child.stdout.take().unwrap();
            let mut process = Process {
                child,
                input,
                output,
                _library: tempfile::NamedTempFile::new().unwrap(),
            };
            process.expect(b'R').await.unwrap();
            process
        }

        fn request(text: &str, path: &Path) -> Vec<String> {
            vec![
                "voicepeak".into(),
                "-s".into(),
                text.into(),
                "-o".into(),
                path.to_str().unwrap().into(),
            ]
        }

        #[tokio::test]
        async fn retains_one_process_only_after_success_and_discards_failed_output() {
            let process = fake_process().await;
            let pid = process.child.id();
            let mut engine = Voicepeak {
                enabled: Some(true),
                process: Some(process),
            };
            for text in ["a", "b"] {
                let file = tempfile::NamedTempFile::new().unwrap();
                assert_eq!(
                    engine
                        .synthesize("unused", &request(text, file.path()), file.path())
                        .await
                        .unwrap(),
                    vec![text.as_bytes()[0]; 4]
                );
                assert_eq!(engine.process.as_ref().unwrap().child.id(), pid);
            }
            let file = tempfile::NamedTempFile::new().unwrap();
            assert!(engine
                .synthesize("unused", &request("bad", file.path()), file.path())
                .await
                .is_err());
            assert!(engine.process.is_none());
        }

        #[tokio::test]
        async fn cancellation_and_child_exit_release_the_owned_process() {
            for text in ["stall", "crash"] {
                let process = fake_process().await;
                let pid = process.child.id().unwrap();
                let mut engine = Voicepeak {
                    enabled: Some(true),
                    process: Some(process),
                };
                let file = tempfile::NamedTempFile::new().unwrap();
                let args = request(text, file.path());
                let result = tokio::time::timeout(
                    Duration::from_millis(50),
                    engine.synthesize("unused", &args, file.path()),
                )
                .await;
                assert!(result.is_err() || result.unwrap().is_err());
                assert!(engine.process.is_none());
                tokio::time::timeout(Duration::from_secs(2), async {
                    while Path::new(&format!("/proc/{pid}")).exists() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("owned child must be killed and reaped");
            }
        }

        #[tokio::test]
        async fn failed_native_start_uses_cli_on_the_next_attempt() {
            let source = tempfile::NamedTempFile::new().unwrap();
            let output = tempfile::NamedTempFile::new().unwrap();
            let mut wav = crate::wav::wav_header(&crate::wav::MOCA_FORMAT);
            wav.extend([1, 2]);
            wav[4..8].copy_from_slice(&38u32.to_le_bytes());
            wav[40..44].copy_from_slice(&2u32.to_le_bytes());
            std::fs::write(source.path(), wav).unwrap();
            let args = vec![
                "cp".into(),
                source.path().to_str().unwrap().into(),
                output.path().to_str().unwrap().into(),
            ];
            // Simulate a binary replacement after the initial version check.
            let mut engine = Voicepeak {
                enabled: Some(true),
                process: None,
            };
            assert!(engine
                .synthesize("/bin/cp", &args, output.path())
                .await
                .is_err());
            assert_eq!(
                engine
                    .synthesize("/bin/cp", &args, output.path())
                    .await
                    .unwrap(),
                [1, 2]
            );
            assert_eq!(engine.enabled, Some(false));
        }
    }
}
