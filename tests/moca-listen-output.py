#!/usr/bin/env python3
"""Observe the listener with real curl/SSE and isolated audio players."""
import json
import os
from pathlib import Path
import queue
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
LISTENER = Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 and not sys.argv[1].startswith('-') else ROOT / 'bin/moca-listen'
STAMP = re.compile(r'^\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}[+-]\d{4}\] moca-listen: ')


class Stream:
    def __init__(self, status=200, content_type='text/event-stream'):
        self.status = status
        self.content_type = content_type
        self.chunks = queue.Queue()
        self.opened = threading.Event()


class ListenerOutput(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name)
        self.streams = queue.Queue()
        self.bodies = []
        self.connections = []
        self.process = None
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                try:
                    stream = owner.streams.get(timeout=3)
                except queue.Empty:
                    self.send_error(503)
                    return
                owner.connections.append(stream)
                self.send_response(stream.status)
                # Deliberately mixed case, as HTTP headers are case-insensitive.
                self.send_header('cOnTeNt-TyPe', stream.content_type)
                self.end_headers()
                self.wfile.flush()
                stream.opened.set()
                try:
                    while True:
                        chunk = stream.chunks.get(timeout=5)
                        if chunk is None:
                            break
                        self.wfile.write(chunk)
                        self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError, queue.Empty):
                    pass

            def do_POST(self):
                body = self.rfile.read(int(self.headers['Content-Length']))
                owner.bodies.append(body)
                if body == b'fetch failure':
                    self.send_error(500)
                    return
                audio = b'RIFF' + b'\xff' * 4 + b'WAVEfmt ' + bytes(24) + b'data' + bytes(8)
                self.send_response(200)
                self.send_header('Content-Length', str(len(audio)))
                self.end_headers()
                self.wfile.write(audio)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.stop)
        self.stdout = self.directory / 'stdout'
        self.stderr = self.directory / 'stderr'
        self.events = self.directory / 'players'
        stub = f'''#!{sys.executable}
import json, os, pathlib, sys, time
args = sys.argv[1:]
if pathlib.Path(sys.argv[0]).name == 'ffplay':
    sys.stdin.buffer.read()
    # Reproduce the real ffplay status output measured with SDL dummy audio.
    if '-nostats' not in args:
        print()
        sys.stderr.write('\\x1b[2K\\r')
with open(os.environ['PLAYER_EVENTS'], 'a') as output:
    output.write(json.dumps(args) + '\\n')
if os.environ.get('PLAYER_GATE'):
    while not pathlib.Path(os.environ['PLAYER_GATE']).exists():
        time.sleep(.01)
if os.environ.get('PLAYER_FAIL'):
    sys.stderr.write('player: device unavailable\\n')
    sys.exit(int(os.environ.get('PLAYER_FAIL_STATUS', '7')))
'''
        for player in ('ffplay', 'afplay', 'pw-play', 'paplay', 'aplay', 'powershell.exe'):
            path = self.directory / player
            path.write_text(stub)
            path.chmod(0o755)
        wslpath = self.directory / 'wslpath'
        wslpath.write_text('#!/bin/sh\nprintf "C:/Temp/moca.wav\\n"\n')
        wslpath.chmod(0o755)

    def stop(self):
        if self.process is not None:
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            self.process.wait(timeout=3)
        for stream in self.connections:
            stream.chunks.put(None)
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)

    def stream(self, **kwargs):
        stream = Stream(**kwargs)
        self.streams.put(stream)
        return stream

    def start(self, player='ffplay', **extra):
        env = dict(os.environ, PATH=f'{self.directory}:' + os.environ['PATH'],
                   MOCA_URL=f'http://127.0.0.1:{self.server.server_port}',
                   MOCA_PLAYER=player, MOCA_RETRY_DELAY='0.05', MOCA_VOLUME='30',
                   PLAYER_EVENTS=str(self.events), TZ='JST-9', **extra)
        with self.stdout.open('wb') as out, self.stderr.open('wb') as err:
            self.process = subprocess.Popen([str(LISTENER)], env=env,
                                            stdout=out, stderr=err, start_new_session=True)

    def read(self, path):
        return path.read_text() if path.exists() else ''

    def wait(self, predicate):
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(.01)
        self.fail(f'timed out; stdout={self.read(self.stdout)!r}; stderr={self.read(self.stderr)!r}')

    def count(self, message, path=None):
        return self.read(path or self.stdout).count(message)

    def assert_clean_states(self):
        for path in (self.stdout, self.stderr):
            text = self.read(path)
            self.assertNotIn('\x1b', text)
            self.assertNotIn('\r', text)
            for line in text.splitlines():
                self.assertTrue(line.strip(), repr(text))
                if 'moca-listen:' in line:
                    self.assertRegex(line, STAMP)
                    self.assertIn('+0900]', line)

    def test_silent_connection_and_reconnection(self):
        first = self.stream()
        second = self.stream()
        self.start()
        self.wait(lambda: self.count('接続しました') == 1)
        self.assertTrue(first.opened.is_set())
        self.assertEqual(self.bodies, [])
        for _ in range(20):
            first.chunks.put(b': keepalive\r\n\r\n')
        first.chunks.put(None)
        self.wait(lambda: self.count('再接続しました') == 1)
        self.assertTrue(second.opened.is_set())
        self.assertEqual(self.count('接続しました'), 2)
        self.assertNotIn('接続しました', self.read(self.stderr))
        self.assert_clean_states()

    def test_http_failure_then_recovery(self):
        bad = self.stream(status=503)
        bad.chunks.put(None)
        good = self.stream()
        self.start()
        self.wait(lambda: self.count('再接続しました') == 1)
        self.assertTrue(good.opened.is_set())
        self.assertEqual(self.count('接続しました'), 1)
        self.assertIn('接続できません', self.read(self.stderr))
        self.assertIn('503', self.read(self.stderr))
        self.assert_clean_states()

    def test_non_sse_response_is_not_connected(self):
        bad = self.stream(content_type='text/html')
        bad.chunks.put(b'data: must not play\n\n')
        bad.chunks.put(None)
        self.stream()
        self.start()
        self.wait(lambda: self.count('再接続しました') == 1)
        self.assertEqual(self.count('接続しました'), 1)
        self.assertEqual(self.bodies, [])
        self.assertIn('SSE', self.read(self.stderr))
        self.assert_clean_states()

    def test_notifications_are_ordered_and_success_follows_player_exit(self):
        stream = self.stream()
        gate = self.directory / 'gate'
        self.start(PLAYER_GATE=str(gate))
        stream.chunks.put(b': ping\n\ndata: private first\ndata: second line\n\n: ping\n\ndata: next\n\ndata: last\n\n')
        self.wait(lambda: self.events.exists())
        self.assertEqual(self.count('通知を受信しました'), 1)
        self.assertEqual(self.count('通知を再生しました'), 0)
        self.assertEqual(self.bodies, [b'private first\nsecond line'])
        gate.touch()
        self.wait(lambda: self.count('通知を再生しました') == 3)
        self.assertEqual(self.bodies, [b'private first\nsecond line', b'next', b'last'])
        messages = [line.split('moca-listen: ')[1] for line in self.read(self.stdout).splitlines()
                    if '通知' in line]
        self.assertEqual(messages, ['通知を受信しました', '通知を再生しました'] * 3)
        self.assertNotIn('private first', self.read(self.stdout) + self.read(self.stderr))
        for args in map(json.loads, self.read(self.events).splitlines()):
            self.assertEqual(args[args.index('-volume') + 1], '30')
        self.assert_clean_states()

    def test_player_failure_keeps_diagnostics_and_continues(self):
        stream = self.stream()
        self.start(PLAYER_FAIL='1')
        stream.chunks.put(b'data: one\n\ndata: two\n\n')
        self.wait(lambda: self.count('通知の再生に失敗しました', self.stderr) == 2)
        self.assertEqual(self.count('通知を受信しました'), 2)
        self.assertEqual(self.count('通知を再生しました'), 0)
        self.assertEqual(self.count('player: device unavailable', self.stderr), 2)
        self.assert_clean_states()

    def test_ffplay_error_with_zero_exit_is_not_success(self):
        stream = self.stream()
        self.start(PLAYER_FAIL='1', PLAYER_FAIL_STATUS='0')
        stream.chunks.put(b'data: broken audio\n\n')
        self.wait(lambda: self.count('通知の再生に失敗しました', self.stderr) == 1)
        self.assertEqual(self.count('通知を再生しました'), 0)
        self.assertIn('player: device unavailable', self.read(self.stderr))
        self.assert_clean_states()

    def test_native_players_and_fetch_failure(self):
        for player in ('afplay', 'pw-play', 'paplay', 'aplay', 'windows-soundplayer'):
            with self.subTest(player=player):
                stream = self.stream()
                self.start(player)
                stream.chunks.put(b'data: native\n\ndata: fetch failure\n\n')
                self.wait(lambda: self.count('通知の再生に失敗しました', self.stderr) == 1)
                self.assertEqual(self.count('通知を受信しました'), 2)
                self.assertEqual(self.count('通知を再生しました'), 1)
                self.assertIn('500', self.read(self.stderr))
                self.assert_clean_states()
                os.killpg(self.process.pid, signal.SIGTERM)
                self.assertEqual(self.process.wait(timeout=3), -signal.SIGTERM)
                stream.chunks.put(None)
                self.process = None

    def test_sigint_stops_playing_listener(self):
        stream = self.stream()
        gate = self.directory / 'gate'
        self.start(PLAYER_GATE=str(gate), TMPDIR=str(self.directory))
        stream.chunks.put(b'data: playing\n\n')
        self.wait(lambda: self.events.exists())
        os.killpg(self.process.pid, signal.SIGINT)
        self.assertEqual(self.process.wait(timeout=3), -signal.SIGINT)
        self.assertEqual(self.count('再接続します'), 0)
        self.wait(lambda: not list(self.directory.glob('moca-listen.*')))

    def test_sigint_stops_idle_listener(self):
        self.stream()
        self.start()
        self.wait(lambda: self.count('接続しました') == 1)
        os.killpg(self.process.pid, signal.SIGINT)
        self.process.wait(timeout=3)
        self.assertNotEqual(self.process.returncode, 0)


if __name__ == '__main__':
    unittest.main()
