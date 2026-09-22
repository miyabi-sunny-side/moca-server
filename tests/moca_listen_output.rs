//! Observe the shipped shell listener with real curl/SSE and isolated players.
#![cfg(unix)]
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::{
        fs::PermissionsExt,
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct Stream {
    chunks: mpsc::Sender<Option<Vec<u8>>>,
    opened: Arc<AtomicBool>,
}
impl Stream {
    fn send(&self, text: &str) {
        self.chunks.send(Some(text.as_bytes().to_vec())).unwrap();
    }
    fn close(&self) {
        let _ = self.chunks.send(None);
    }
    fn opened(&self) -> bool {
        self.opened.load(Ordering::SeqCst)
    }
}
struct Reply {
    status: u16,
    content_type: &'static str,
    chunks: mpsc::Receiver<Option<Vec<u8>>>,
    opened: Arc<AtomicBool>,
}
#[derive(Default)]
struct Server {
    streams: Mutex<VecDeque<Reply>>,
    bodies: Mutex<Vec<String>>,
    stopped: AtomicBool,
}
impl Server {
    fn serve(&self, mut socket: TcpStream) -> std::io::Result<()> {
        socket.set_read_timeout(Some(Duration::from_secs(2)))?;
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut input = BufReader::new(socket.try_clone()?);
        let mut first = String::new();
        input.read_line(&mut first)?;
        let mut length = 0;
        loop {
            let mut header = String::new();
            if input.read_line(&mut header)? == 0 || header == "\r\n" {
                break;
            }
            if let Some((key, value)) = header.split_once(':') {
                if key.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
        }
        if first.starts_with("GET ") {
            let reply = self.streams.lock().unwrap().pop_front();
            let Some(reply) = reply else {
                return socket.write_all(b"HTTP/1.0 503 Unavailable\r\nContent-Length: 0\r\n\r\n");
            };
            // Keep mixed-case headers: detecting a non-SSE response must be case-insensitive.
            write!(
                socket,
                "HTTP/1.0 {} Fixture\r\ncOnTeNt-TyPe: {}\r\n\r\n",
                reply.status, reply.content_type
            )?;
            socket.flush()?;
            reply.opened.store(true, Ordering::SeqCst);
            while !self.stopped.load(Ordering::SeqCst) {
                match reply.chunks.recv_timeout(Duration::from_millis(50)) {
                    Ok(Some(bytes)) => {
                        socket.write_all(&bytes)?;
                        socket.flush()?;
                    }
                    Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        } else {
            assert!(first.starts_with("POST "), "{first}");
            let mut body = vec![0; length];
            input.read_exact(&mut body)?;
            self.bodies
                .lock()
                .unwrap()
                .push(String::from_utf8(body.clone()).unwrap());
            if body == b"fetch failure" {
                socket.write_all(b"HTTP/1.0 500 Fixture\r\nContent-Length: 0\r\n\r\n")?;
            } else {
                let audio = [
                    b"RIFF".as_slice(),
                    &[255; 4],
                    b"WAVEfmt ",
                    &[0; 24],
                    b"data",
                    &[0; 8],
                ]
                .concat();
                write!(
                    socket,
                    "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n",
                    audio.len()
                )?;
                socket.write_all(&audio)?;
            }
        }
        Ok(())
    }
}

struct Listener {
    directory: tempfile::TempDir,
    server: Arc<Server>,
    server_thread: Option<JoinHandle<()>>,
    port: u16,
    child: Option<Child>,
}
impl Listener {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        for player in [
            "ffplay",
            "afplay",
            "pw-play",
            "paplay",
            "aplay",
            "powershell.exe",
        ] {
            executable(
                &directory.path().join(player),
                include_str!("support/player.sh"),
            );
        }
        executable(
            &directory.path().join("wslpath"),
            "#!/bin/sh\nprintf 'C:/Temp/moca.wav\\n'\n",
        );
        let server = Arc::new(Server::default());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = server.clone();
        let server_thread = thread::spawn(move || {
            let mut connections = Vec::new();
            while !state.stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        let state = state.clone();
                        connections.push(thread::spawn(move || {
                            let _ = state.serve(socket);
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
            for connection in connections {
                connection.join().unwrap();
            }
        });
        Self {
            directory,
            server,
            server_thread: Some(server_thread),
            port,
            child: None,
        }
    }
    fn path(&self, file: &str) -> PathBuf {
        self.directory.path().join(file)
    }
    fn stream(&self, status: u16, content_type: &'static str) -> Stream {
        let (chunks, receiver) = mpsc::channel();
        let opened = Arc::new(AtomicBool::new(false));
        self.server.streams.lock().unwrap().push_back(Reply {
            status,
            content_type,
            chunks: receiver,
            opened: opened.clone(),
        });
        Stream { chunks, opened }
    }
    fn sse(&self) -> Stream {
        self.stream(200, "text/event-stream")
    }
    fn start(&mut self, player: &str, extra: &[(&str, &str)]) {
        assert!(self.child.is_none());
        let listener = std::env::var_os("MOCA_LISTENER_UNDER_TEST")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/moca-listen"));
        let path = std::env::join_paths(
            std::iter::once(self.directory.path().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        self.child = Some(
            Command::new(listener.canonicalize().unwrap())
                .process_group(0)
                .env("PATH", path)
                .env("MOCA_URL", format!("http://127.0.0.1:{}", self.port))
                .env("MOCA_PLAYER", player)
                .env("MOCA_RETRY_DELAY", "0.05")
                .env("MOCA_VOLUME", "30")
                .env("PLAYER_EVENTS", self.path("players"))
                .env("TMPDIR", self.directory.path())
                .env("TZ", "JST-9")
                .env_remove("PLAYER_GATE")
                .env_remove("PLAYER_FAIL")
                .env_remove("PLAYER_FAIL_STATUS")
                .envs(extra.iter().copied())
                .stdout(File::create(self.path("stdout")).unwrap())
                .stderr(File::create(self.path("stderr")).unwrap())
                .spawn()
                .unwrap(),
        );
    }
    fn read(&self, file: &str) -> String {
        fs::read_to_string(self.path(file)).unwrap_or_default()
    }
    fn count(&self, text: &str, file: &str) -> usize {
        self.read(file).matches(text).count()
    }
    fn bodies(&self) -> Vec<String> {
        self.server.bodies.lock().unwrap().clone()
    }
    fn wait(&self, ready: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(4);
        while !ready() {
            assert!(
                Instant::now() < end,
                "timeout; stdout={:?}; stderr={:?}",
                self.read("stdout"),
                self.read("stderr")
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn clean_states(&self) {
        for file in ["stdout", "stderr"] {
            let output = self.read(file);
            assert!(!output.contains(['\x1b', '\r']), "{output:?}");
            for line in output.lines() {
                assert!(!line.trim().is_empty(), "{output:?}");
                if line.contains("moca-listen:") {
                    let (stamp, _) = line
                        .strip_prefix('[')
                        .unwrap()
                        .split_once("] moca-listen: ")
                        .unwrap();
                    assert!(stamp.ends_with("+0900"), "{line}");
                    let parsed =
                        chrono::DateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S%z").unwrap();
                    assert_eq!(parsed.format("%Y-%m-%d %H:%M:%S%z").to_string(), stamp);
                }
            }
        }
    }
    fn signal(&self, signal: &str) {
        signal_group(self.child.as_ref().unwrap().id(), signal);
    }
    fn wait_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "listener failed to stop");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            signal_group(child.id(), "TERM");
            let deadline = Instant::now() + Duration::from_secs(3);
            while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if child.try_wait().unwrap().is_none() {
                signal_group(child.id(), "KILL");
            }
            let _ = child.wait();
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
        self.server.stopped.store(true, Ordering::SeqCst);
        if let Some(thread) = self.server_thread.take() {
            let _ = thread.join();
        }
    }
}
fn signal_group(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args(["-s", signal, "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn silent_connection_and_reconnection() {
    let mut l = Listener::new();
    let first = l.sse();
    let second = l.sse();
    l.start("ffplay", &[]);
    l.wait(|| l.count("接続しました", "stdout") == 1);
    assert!(first.opened());
    assert!(l.bodies().is_empty());
    for _ in 0..20 {
        first.send(": keepalive\r\n\r\n");
    }
    first.close();
    l.wait(|| l.count("再接続しました", "stdout") == 1);
    assert!(second.opened());
    assert_eq!(l.count("接続しました", "stdout"), 2);
    assert!(!l.read("stderr").contains("接続しました"));
    l.clean_states();
}
#[test]
fn http_failure_then_recovery() {
    let mut l = Listener::new();
    let bad = l.stream(503, "text/event-stream");
    bad.close();
    let good = l.sse();
    l.start("ffplay", &[]);
    l.wait(|| l.count("再接続しました", "stdout") == 1);
    assert!(good.opened());
    assert_eq!(l.count("接続しました", "stdout"), 1);
    assert!(l.read("stderr").contains("接続できません"));
    assert!(l.read("stderr").contains("503"));
    l.clean_states();
}
#[test]
fn non_sse_response_is_not_connected() {
    let mut l = Listener::new();
    let bad = l.stream(200, "text/html");
    bad.send("data: must not play\n\n");
    bad.close();
    let _good = l.sse();
    l.start("ffplay", &[]);
    l.wait(|| l.count("再接続しました", "stdout") == 1);
    assert_eq!(l.count("接続しました", "stdout"), 1);
    assert!(l.bodies().is_empty());
    assert!(l.read("stderr").contains("SSE"));
    l.clean_states();
}
#[test]
fn notifications_are_ordered_and_success_follows_player_exit() {
    let mut l = Listener::new();
    let stream = l.sse();
    let gate = l.path("gate");
    l.start("ffplay", &[("PLAYER_GATE", gate.to_str().unwrap())]);
    stream.send(": ping\n\ndata: private first\ndata: second line\n\n: ping\n\ndata: next\n\ndata: last\n\n");
    l.wait(|| l.path("players").exists());
    assert_eq!(l.count("通知を受信しました", "stdout"), 1);
    assert_eq!(l.count("通知を再生しました", "stdout"), 0);
    assert_eq!(l.bodies(), ["private first\nsecond line"]);
    fs::write(gate, "").unwrap();
    l.wait(|| l.count("通知を再生しました", "stdout") == 3);
    assert_eq!(l.bodies(), ["private first\nsecond line", "next", "last"]);
    let output = l.read("stdout");
    let messages: Vec<_> = output
        .lines()
        .filter(|line| line.contains("通知"))
        .map(|line| line.split_once("moca-listen: ").unwrap().1)
        .collect();
    assert_eq!(
        messages,
        ["通知を受信しました", "通知を再生しました"].repeat(3)
    );
    assert!(!(output + &l.read("stderr")).contains("private first"));
    let records = l.read("players");
    for record in records.split("\0\0").filter(|s| !s.is_empty()) {
        let args: Vec<_> = record.split('\0').collect();
        assert_eq!(
            args[args.iter().position(|a| *a == "-volume").unwrap() + 1],
            "30"
        );
    }
    l.clean_states();
}
#[test]
fn player_failure_keeps_diagnostics_and_continues() {
    let mut l = Listener::new();
    let stream = l.sse();
    l.start("ffplay", &[("PLAYER_FAIL", "1")]);
    stream.send("data: one\n\ndata: two\n\n");
    l.wait(|| l.count("通知の再生に失敗しました", "stderr") == 2);
    assert_eq!(l.count("通知を受信しました", "stdout"), 2);
    assert_eq!(l.count("通知を再生しました", "stdout"), 0);
    assert_eq!(l.count("player: device unavailable", "stderr"), 2);
    l.clean_states();
}
#[test]
fn ffplay_error_with_zero_exit_is_not_success() {
    let mut l = Listener::new();
    let stream = l.sse();
    l.start(
        "ffplay",
        &[("PLAYER_FAIL", "1"), ("PLAYER_FAIL_STATUS", "0")],
    );
    stream.send("data: broken audio\n\n");
    l.wait(|| l.count("通知の再生に失敗しました", "stderr") == 1);
    assert_eq!(l.count("通知を再生しました", "stdout"), 0);
    assert!(l.read("stderr").contains("player: device unavailable"));
    l.clean_states();
}
#[test]
fn native_players_and_fetch_failure() {
    for player in [
        "afplay",
        "pw-play",
        "paplay",
        "aplay",
        "windows-soundplayer",
    ] {
        let mut l = Listener::new();
        let stream = l.sse();
        l.start(player, &[]);
        stream.send("data: native\n\ndata: fetch failure\n\n");
        l.wait(|| l.count("通知の再生に失敗しました", "stderr") == 1);
        assert_eq!(l.count("通知を受信しました", "stdout"), 2);
        assert_eq!(l.count("通知を再生しました", "stdout"), 1);
        assert!(l.read("stderr").contains("500"));
        l.clean_states();
        l.signal("TERM");
        assert_eq!(l.wait_exit().signal(), Some(15));
        stream.close();
    }
}
#[test]
fn sigint_stops_playing_listener() {
    let mut l = Listener::new();
    let stream = l.sse();
    let gate = l.path("gate");
    let tmp = l.directory.path().to_path_buf();
    l.start(
        "ffplay",
        &[
            ("PLAYER_GATE", gate.to_str().unwrap()),
            ("TMPDIR", tmp.to_str().unwrap()),
        ],
    );
    stream.send("data: playing\n\n");
    l.wait(|| l.path("players").exists());
    l.signal("INT");
    assert_eq!(l.wait_exit().signal(), Some(2));
    assert_eq!(l.count("再接続します", "stdout"), 0);
    l.wait(|| {
        fs::read_dir(&tmp).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("moca-listen.")
        })
    });
}
#[test]
fn sigint_stops_idle_listener() {
    let mut l = Listener::new();
    let _stream = l.sse();
    l.start("ffplay", &[]);
    l.wait(|| l.count("接続しました", "stdout") == 1);
    l.signal("INT");
    assert!(!l.wait_exit().success());
}
