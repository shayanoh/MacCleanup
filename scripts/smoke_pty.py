"""End-to-end smoke test: drive the real binary through a pseudo-terminal.

A tiny terminal emulator rebuilds the visible screen (ratatui only writes
changed cells, so the raw byte stream is not enough). Uses a throwaway HOME,
so nothing real is touched.
"""

import fcntl
import glob
import os
import pty
import select
import shutil
import struct
import sys
import tempfile
import termios
import time
import unicodedata

BIN = sys.argv[1]
ROWS, COLS = 30, 100


class Screen:
    def __init__(self):
        self.grid = [[" "] * COLS for _ in range(ROWS)]
        self.r = 0
        self.c = 0
        self.pending = ""

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.grid)

    def put(self, ch):
        w = 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1
        if 0 <= self.r < ROWS and 0 <= self.c < COLS:
            self.grid[self.r][self.c] = ch
            if w == 2 and self.c + 1 < COLS:
                self.grid[self.r][self.c + 1] = " "
        self.c += w

    def feed(self, data):
        s = self.pending + data
        self.pending = ""
        i = 0
        n = len(s)
        while i < n:
            ch = s[i]
            if ch == "\x1b":
                if i + 1 >= n:
                    self.pending = s[i:]
                    return
                nxt = s[i + 1]
                if nxt == "[":
                    j = i + 2
                    while j < n and not ("@" <= s[j] <= "~"):
                        j += 1
                    if j >= n:
                        self.pending = s[i:]
                        return
                    self.csi(s[i + 2 : j], s[j])
                    i = j + 1
                elif nxt == "]":
                    j = i + 2
                    while j < n and s[j] != "\x07":
                        j += 1
                    if j >= n:
                        self.pending = s[i:]
                        return
                    i = j + 1
                elif nxt in "()":
                    i += 3
                else:
                    i += 2
            elif ch == "\r":
                self.c = 0
                i += 1
            elif ch == "\n":
                self.r += 1
                i += 1
            elif ch == "\b":
                self.c = max(0, self.c - 1)
                i += 1
            elif ch == "\x07":
                i += 1
            else:
                self.put(ch)
                i += 1

    def csi(self, params, final):
        nums = [int(p) if p.isdigit() else 0 for p in params.lstrip("?").split(";")] if params else []
        a = nums[0] if nums else 0
        if final in "Hf":
            self.r = (nums[0] if len(nums) > 0 and nums[0] else 1) - 1
            self.c = (nums[1] if len(nums) > 1 and nums[1] else 1) - 1
        elif final == "J":
            if a == 2:
                self.grid = [[" "] * COLS for _ in range(ROWS)]
            elif a == 0:
                for c in range(self.c, COLS):
                    self.grid[self.r][c] = " "
                for r in range(self.r + 1, ROWS):
                    self.grid[r] = [" "] * COLS
        elif final == "K":
            if a == 0:
                for c in range(self.c, COLS):
                    self.grid[self.r][c] = " "
            elif a == 2:
                self.grid[self.r] = [" "] * COLS
        elif final == "A":
            self.r = max(0, self.r - (a or 1))
        elif final == "B":
            self.r = min(ROWS - 1, self.r + (a or 1))
        elif final == "C":
            self.c = min(COLS - 1, self.c + (a or 1))
        elif final == "D":
            self.c = max(0, self.c - (a or 1))
        elif final == "G":
            self.c = max(0, (a or 1) - 1)


home = tempfile.mkdtemp(prefix="mc-smoke-")
os.makedirs(os.path.join(home, "proj"))
with open(os.path.join(home, "proj", "big.bin"), "wb") as f:
    f.write(b"x" * 200_000)
with open(os.path.join(home, "note.txt"), "wb") as f:
    f.write(b"hello")

pid, fd = pty.fork()
if pid == 0:
    env = dict(os.environ, HOME=home, TERM="xterm-256color")
    os.execve(BIN, [BIN, "--no-animation"], env)

fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
screen = Screen()
raw_tail = ""


def pump(seconds):
    global raw_tail
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return False
            if not data:
                return False
            text = data.decode("utf-8", errors="replace")
            raw_tail = (raw_tail + text)[-600:]
            screen.feed(text)
    return True


def cleanup(code):
    try:
        os.kill(pid, 9)
    except OSError:
        pass
    shutil.rmtree(home, ignore_errors=True)
    sys.exit(code)


def fail(msg):
    print(f"FAIL: {msg}\n--- screen ---\n{screen.text()}\n--- end ---")
    cleanup(1)


def wait_for(text, timeout=6.0):
    end = time.time() + timeout
    while time.time() < end:
        if text in screen.text():
            return
        if not pump(0.1):
            break
    fail(f"never saw {text!r}")


def wait_gone(text, timeout=4.0):
    end = time.time() + timeout
    while time.time() < end:
        if text not in screen.text():
            return
        pump(0.1)
    fail(f"{text!r} did not go away")


def send(data):
    os.write(fd, data)


def step(name):
    print("step:", name)


step("home screen")
wait_for("Free")
wait_for("Biggest folders")
wait_for("Scan complete")

step("explorer, wait for the scan, jump to the top row (the biggest: proj)")
send(b"2")
wait_for("marked: 0 items")
wait_for("proj")
pump(1.0)
send(b"\x1b[H")  # Home
pump(0.4)

if os.environ.get("DUMP"):
    print("=== HOME ==="); send(b"1"); pump(0.5); print(screen.text())
    send(b"2"); pump(0.5)
    print("=== EXPLORER (bars) ==="); print(screen.text())
    send(b"t"); pump(0.5)
    print("=== EXPLORER (treemap) ==="); print(screen.text())
    send(b"t"); send(b"t"); pump(0.3)

step("mark proj and open the review dialog")
send(b" ")
wait_for("marked: 1 items")
send(b"d")
wait_for("Review before moving")
wait_for("proj")
wait_for("[ Enter ]")

step("cancel keeps everything")
send(b"\x1b")
wait_gone("Review before moving")
assert os.path.exists(os.path.join(home, "proj", "big.bin")), "cancel must not delete"
assert "marked: 1 items" in screen.text(), "marks survive a cancelled review"

step("review again and confirm the soft delete")
send(b"d")
wait_for("Review before moving")
send(b"\r")
wait_for("pending, not freed yet")
assert not os.path.exists(os.path.join(home, "proj")), "proj should have been moved"
payloads = glob.glob(os.path.join(home, "ToBeDeleted", "items", "*", "payload"))
assert len(payloads) == 1, f"expected one held payload, got {payloads}"

step("holding screen and restore")
send(b"\r")  # dismiss the notice
wait_gone("pending, not freed yet")
send(b"3")
wait_for("1 items")
send(b"r")
wait_for("Restored")
assert os.path.exists(os.path.join(home, "proj", "big.bin")), "restore failed"

step("irreversible path asks twice and needs y")
send(b"2")
wait_for("marked: 0 items")
pump(0.8)
send(b"\x1b[H")
pump(0.3)
send(b" ")
wait_for("marked: 1 items")
send(b"d")
wait_for("Review before moving")
send(b"x")
wait_for("cannot be undone")
send(b"\r")  # Enter must NOT confirm an irreversible delete
pump(0.6)
assert os.path.exists(os.path.join(home, "proj", "big.bin")), "Enter must not confirm irreversible"
assert "cannot be undone" in screen.text(), "dialog should still be open"
send(b"y")
wait_for("Congratulations")
assert not os.path.exists(os.path.join(home, "proj")), "proj should be gone for good"
assert not glob.glob(os.path.join(home, "ToBeDeleted", "items", "*")), "holding should be empty"

step("quit")
send(b"\r")
wait_gone("Congratulations")
send(b"q")
end = time.time() + 4
status = None
while time.time() < end:
    done, st = os.waitpid(pid, os.WNOHANG)
    if done:
        status = st
        break
    pump(0.1)
if status is None:
    fail("process did not exit after q")
assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, f"bad exit status {status}"
assert "panicked" not in raw_tail and "panicked" not in screen.text(), "panic message in output"
print("SMOKE OK")
shutil.rmtree(home, ignore_errors=True)
