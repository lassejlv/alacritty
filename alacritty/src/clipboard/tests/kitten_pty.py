"""Connect the real kitten client to the test terminal over a controlling PTY."""
import base64
import errno
import fcntl
import json
import os
import pty
import select
import subprocess
import sys
import termios
import time
import tty

master, slave = pty.openpty()
tty.setraw(slave)

def setup():
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

client = subprocess.Popen(sys.argv[1:], stdin=slave, stdout=slave, stderr=slave, preexec_fn=setup)
os.close(slave)
deadline = time.monotonic() + 15
pending = b''
try:
    while time.monotonic() < deadline:
        ready, _, _ = select.select([master, sys.stdin], [], [], 0.05)
        if master in ready:
            try:
                data = os.read(master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                data = b''
            if data:
                print(json.dumps({'data': base64.b64encode(data).decode()}), flush=True)
        if sys.stdin in ready:
            chunk = os.read(sys.stdin.fileno(), 65536)
            if not chunk:
                break
            pending += chunk
            while b'\n' in pending:
                line, pending = pending.split(b'\n', 1)
                os.write(master, base64.b64decode(json.loads(line)['reply']))
        if client.poll() is not None:
            print(json.dumps({'exit': client.returncode}), flush=True)
            sys.exit(client.returncode)
    raise TimeoutError('kitten clipboard did not finish in 15 seconds')
finally:
    if client.poll() is None:
        client.terminate()
        client.wait(timeout=5)
    os.close(master)
