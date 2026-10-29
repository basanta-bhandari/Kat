"""Linux PTY integration checks; simulate protocol replies, not real terminal rendering."""
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time


def run_case(binary, label, reply, supported, graphics="auto", arguments=(), shell=None):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 800, 384))
    original = termios.tcgetattr(slave)

    def session():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    environment = dict(os.environ, TERM="xterm-kitty", KITTY_WINDOW_ID="123", SHELL="/bin/bash",
                       PATH="/nonexistent/no-fish-needed", FORGETUI_GRAPHICS=graphics, FORGETUI_QUALITY="low")
    command = [binary, *arguments]
    if shell:
        command = [shell, "-c", 'exec "$@"', "forge-test", *command]
    process = subprocess.Popen(command, stdin=slave, stdout=slave,
                               stderr=slave, env=environment, preexec_fn=session)
    output = bytearray()
    queried = False
    opened = False
    quit_sent = False
    deadline = time.monotonic() + 10
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                output.extend(os.read(master, 65536))
            if not queried and b"a=q,t=d,f=24;AAAA" in output:
                queried = True
                if reply:
                    os.write(master, reply)
            if b"\x1b[6n" in output:
                os.write(master, b"\x1b[1;1R")
                output[:] = output.replace(b"\x1b[6n", b"")
            if supported and not opened and b"Projects" in output:
                os.write(master, b"n")
                opened = True
            pixel_output = b"\x1b_Ga=T" in output
            if supported and pixel_output and not quit_sent:
                os.write(master, b"q")
                quit_sent = True
            if process.poll() is not None:
                break
        assert process.poll() is not None, f"{label}: process did not exit"
        if supported:
            assert process.returncode == 0, f"{label}: {bytes(output[-500:])!r}"
            assert opened and quit_sent, f"{label}: no pixel viewport"
        else:
            assert process.returncode != 0, f"{label}: unsupported terminal was accepted"
            assert b"\x1b[?1049h" not in output, f"{label}: UI opened before verification"
        assert termios.tcgetattr(slave) == original, f"{label}: terminal mode was not restored"
        print(f"PASS {label}")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/forgetui")
    kitty_reply = b"\x1bP>|kitty(0.42.0)\x1b\\\x1b_Gi=48391;OK\x1b\\\x1b[?62;1;2c"
    run_case(binary, "Kitty accepted from bash without fish on PATH", kitty_reply, True, shell="/bin/bash")
    run_case(binary, "Kitty accepted from sh without fish on PATH", kitty_reply, True, shell="/bin/sh")
    run_case(binary, "Sixel-only terminal rejected", b"\x1b[?62;1;4;6c", False)
    run_case(binary, "other terminal with Kitty protocol rejected", b"\x1bP>|WezTerm(20261001)\x1b\\\x1b_Gi=48391;OK\x1b\\\x1b[?62;1;2c", False)
    run_case(binary, "Kitty graphics without Kitty identity rejected", b"\x1b_Gi=48391;OK\x1b\\\x1b[?62;1;2c", False)
    run_case(binary, "Kitty identity without pixel graphics rejected", b"\x1bP>|kitty(0.42.0)\x1b\\\x1b[?62;1;2c", False)
    run_case(binary, "unsupported terminal rejected despite Kitty environment", b"\x1b[?62;1;2c", False)
    run_case(binary, "no reply rejected", None, False)
    run_case(binary, "ASCII override rejected", None, False, graphics="ascii")
    run_case(binary, "Sixel override rejected", None, False, graphics="sixel")
    run_case(binary, "legacy text preview rejected", None, False, arguments=("--render",))
