#!/usr/bin/env python3
"""crop.py in.png out.png x y w h — pure-python PNG crop (RGB/RGBA, 8-bit)."""
import sys, zlib, struct
inp, outp = sys.argv[1], sys.argv[2]
x, y, w, h = map(int, sys.argv[3:7])
data = open(inp, 'rb').read()
pos = 8; idat = b''; W=H=ct=None
while pos < len(data):
    ln = struct.unpack('>I', data[pos:pos+4])[0]; typ = data[pos+4:pos+8]
    chunk = data[pos+8:pos+8+ln]
    if typ == b'IHDR': W,H,bd,ct = struct.unpack('>IIBB', chunk[:10])
    elif typ == b'IDAT': idat += chunk
    pos += 12+ln
ch = {0:1,2:3,4:2,6:4}[ct]
raw = zlib.decompress(idat)
stride = W*ch
prev = bytearray(stride); rows=[]; p=0
for _ in range(H):
    f = raw[p]; p+=1
    line = bytearray(raw[p:p+stride]); p+=stride
    for i in range(stride):
        a = line[i-ch] if i>=ch else 0
        b = prev[i]; c = prev[i-ch] if i>=ch else 0
        if f==1: line[i]=(line[i]+a)&255
        elif f==2: line[i]=(line[i]+b)&255
        elif f==3: line[i]=(line[i]+(a+b)//2)&255
        elif f==4:
            pa=abs(b-c); pb=abs(a-c); pc=abs(a+b-2*c)
            pr = a if pa<=pb and pa<=pc else (b if pb<=pc else c)
            line[i]=(line[i]+pr)&255
    rows.append(line); prev=line
x2, y2 = min(x+w, W), min(y+h, H)
out_rows = b''.join(b'\x00'+bytes(rows[yy][x*ch:x2*ch]) for yy in range(y, y2))
def chunk_out(typ, payload):
    return struct.pack('>I', len(payload)) + typ + payload + struct.pack('>I', zlib.crc32(typ+payload)&0xffffffff)
ihdr = struct.pack('>IIBBBBB', x2-x, y2-y, 8, ct, 0, 0, 0)
png = b'\x89PNG\r\n\x1a\n' + chunk_out(b'IHDR', ihdr) + chunk_out(b'IDAT', zlib.compress(out_rows, 6)) + chunk_out(b'IEND', b'')
open(outp,'wb').write(png)
print(f"cropped {x2-x}x{y2-y} -> {outp}")
