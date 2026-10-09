#!/usr/bin/env python3
"""Self-contained Kitty graphics visual/transport check. Run inside Alacritty.

Uses only the Python standard library. --report writes probe results for automated QA.
"""
import argparse
import base64
import ctypes
import json
import mmap
import os
from pathlib import Path
import select
import struct
import sys
import tempfile
import termios
import time
import tty
import zlib


def send(control, data=b''):
    sys.stdout.buffer.write(b'\x1b_G' + control.encode() + b';' + base64.b64encode(data) + b'\x1b\\')
    sys.stdout.buffer.flush()


def text(row, col, label):
    sys.stdout.write(f'\x1b[{row};{col}H{label}')
    sys.stdout.flush()


def png(width, height, rgba):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    rows = b''.join(b'\0' + rgba[y * width * 4:(y + 1) * width * 4] for y in range(height))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b'')


def read_reply():
    data = b''
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        if select.select([sys.stdin], [], [], 0.1)[0]:
            data += os.read(sys.stdin.fileno(), 4096)
            if b'\x1b\\' in data:
                return data.decode('ascii', 'replace')
    return data.decode('ascii', 'replace')


def probe(control, data):
    send(control, data)
    return read_reply()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', type=Path)
    parser.add_argument('--seconds', type=float, default=60)
    args = parser.parse_args()
    old = termios.tcgetattr(sys.stdin)
    checks = {}
    try:
        tty.setraw(sys.stdin)
        checks['direct_query'] = probe('a=q,i=900,f=24,s=1,v=1', b'\xff\0\0')
        checks['zlib_query'] = probe('a=q,i=901,f=32,s=1,v=1,o=z', zlib.compress(b'\xff\0\0\xff'))
        with tempfile.TemporaryDirectory(prefix='tty-graphics-protocol-') as directory:
            image_file = Path(directory) / 'image.rgba'
            image_file.write_bytes(b'head\xff\0\0\xfftail')
            checks['file_range'] = probe('a=q,i=902,f=32,s=1,v=1,t=f,S=4,O=4', os.fsencode(image_file))
            checks['temporary_file'] = probe('a=q,i=903,f=32,s=1,v=1,t=t,S=4,O=4', os.fsencode(image_file))
            checks['temporary_deleted'] = not image_file.exists()
        if sys.platform != 'win32':
            libc = ctypes.CDLL(None, use_errno=True)
            # shm_open is variadic on Darwin: declare only its fixed arguments.
            libc.shm_open.argtypes = [ctypes.c_char_p, ctypes.c_int]
            libc.shm_unlink.argtypes = [ctypes.c_char_p]
            name = f'/alacritty-kitty-{os.getpid()}'.encode()
            fd = libc.shm_open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
            if fd >= 0:
                try:
                    os.ftruncate(fd, 8)
                    with mmap.mmap(fd, 8) as view:
                        view[:] = b'head\xff\0\0\xff'
                        checks['shared_memory'] = probe('a=q,i=904,f=32,s=1,v=1,t=s,S=4,O=4', name)
                finally:
                    os.close(fd)
                    libc.shm_unlink(name)
        checks['passed'] = all(';OK' in value for value in checks.values() if isinstance(value, str)) and checks.get('temporary_deleted', False) and 'shared_memory' in checks
        if args.report:
            args.report.write_text(json.dumps(checks, indent=2))
        send('a=d,d=A,q=2')
        sys.stdout.write('\x1b[2J\x1b[H\x1b[?25l')
        columns = os.get_terminal_size().columns
        text(1, 2, f'Kitty graphics pane PID {os.getpid()}' if columns < 70 else f'Kitty graphics: PNG / RGBA / zlib / layers / placeholders / animation  PID {os.getpid()}')
        pixels = bytes(channel for y in range(80) for x in range(120)
                       for channel in (x * 255 // 119, y * 255 // 79, 180, 255))
        image = png(120, 80, pixels)
        text(3, 2, 'Chunked PNG')
        text(4, 2, '')
        encoded = base64.b64encode(image)
        for offset in range(0, len(encoded), 4096):
            chunk = encoded[offset:offset + 4096]
            more = int(offset + len(chunk) < len(encoded))
            control = f'a=T,f=100,i=10,c=20,r=6,C=1,q=2,m={more}' if offset == 0 else f'm={more},q=2'
            sys.stdout.buffer.write(b'\x1b_G' + control.encode() + b';' + chunk + b'\x1b\\')
        sys.stdout.buffer.flush()
        if columns < 70:
            text(12, 2, 'Independent animation')
            text(13, 2, '')
            send('a=T,i=20,f=32,s=1,v=1,c=16,r=4,C=1,q=2', bytes([240, 60, 60, 255]))
            send('a=f,i=20,f=32,s=1,v=1,z=450,q=2', bytes([40, 120, 250, 255]))
            send('a=a,i=20,r=1,z=450,s=3,v=1,q=2')
            text(20, 2, 'Transport probes: ' + ('PASS' if checks['passed'] else 'FAIL'))
            time.sleep(args.seconds)
            return
        text(3, 27, 'Zlib RGBA crop')
        text(4, 27, '')
        send('a=T,i=11,f=32,s=120,v=80,o=z,x=30,y=20,w=60,h=40,c=20,r=6,C=1,q=2', zlib.compress(pixels))
        text(3, 52, 'Behind text')
        text(5, 52, '')
        send('a=T,i=12,f=32,s=1,v=1,c=16,r=4,z=-1,C=1,q=2', bytes([20, 140, 80, 255]))
        text(6, 56, '\x1b[97mTEXT\x1b[0m')
        text(3, 75, 'Above text / alpha')
        text(6, 77, 'OVERLAP')
        text(5, 75, '')
        send('a=T,i=13,f=32,s=1,v=1,c=16,r=4,z=2,C=1,q=2', bytes([250, 100, 40, 180]))
        text(12, 2, 'Animation (red / blue)')
        text(13, 2, '')
        send('a=T,i=20,f=32,s=1,v=1,c=16,r=4,C=1,q=2', bytes([240, 60, 60, 255]))
        send('a=f,i=20,f=32,s=1,v=1,z=450,q=2', bytes([40, 120, 250, 255]))
        send('a=a,i=20,r=1,z=450,s=3,v=1,q=2')
        text(12, 27, 'Unicode placeholders')
        send('a=T,i=21,p=1,U=1,f=32,s=120,v=80,c=12,r=4,C=1,q=2', pixels)
        marks = [0x305, 0x30D, 0x30E, 0x310, 0x312, 0x33D, 0x33E, 0x33F, 0x346, 0x34A, 0x34B, 0x34C]
        for row in range(4):
            text(13 + row, 27, '\x1b[38;5;21m' + ''.join(chr(0x10EEEE) + chr(marks[row]) + chr(marks[col]) for col in range(12)) + '\x1b[0m')
        text(12, 52, 'Relative placement')
        text(14, 52, '')
        send('a=T,i=22,p=1,f=32,s=1,v=1,C=1,q=2', bytes([0, 0, 0, 0]))
        send('a=T,i=23,p=1,P=22,Q=1,H=2,V=0,f=32,s=120,v=80,c=16,r=4,q=2', pixels)
        text(20, 2, 'Transport probes: ' + ('PASS' if checks['passed'] else 'FAILED (see report)'))
        text(22, 2, 'Use split panes to verify independent images and clipping. Ctrl+C exits.')
        time.sleep(args.seconds)
    finally:
        sys.stdout.write('\x1b[?25h\x1b[0m')
        sys.stdout.flush()
        termios.tcsetattr(sys.stdin, termios.TCSANOW, old)


if __name__ == '__main__':
    main()
