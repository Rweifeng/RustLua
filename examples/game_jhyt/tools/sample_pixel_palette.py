"""Sample the dominant color bins of a pixel-art PNG (stdlib only, no Pillow).

Usage: py sample_pixel_palette.py [--step N] [--top K] <image.png>...
Bins are 16-wide per channel, so a reported `#e0a020` covers real colors in
e0-ef / a0-af / 20-2f. Sampling skips pixels; pass --step 1 for an exact count.
"""
import struct
import sys
import zlib
from collections import Counter


def decode_png(path):
    data = open(path, 'rb').read()
    if data[:8] != b'\x89PNG\r\n\x1a\n':
        raise SystemExit('%s: not a PNG' % path)
    pos = 8
    idat = bytearray()
    header = palette = None
    while pos < len(data):
        length = struct.unpack('>I', data[pos:pos + 4])[0]
        kind = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + length]
        if kind == b'IHDR':
            header = struct.unpack('>IIBBBBB', body)
        elif kind == b'IDAT':
            idat += body
        elif kind == b'PLTE':
            palette = body
        pos += 12 + length
    width, height, depth, color, _comp, _filt, interlace = header
    if depth != 8 or interlace != 0:
        raise SystemExit('%s: only 8-bit non-interlaced PNG supported' % path)
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color]
    raw = zlib.decompress(bytes(idat))
    stride = width * channels
    out = bytearray()
    prev = bytearray(stride)
    offset = 0
    for _y in range(height):
        filter_type = raw[offset]
        offset += 1
        line = bytearray(raw[offset:offset + stride])
        offset += stride
        for x in range(stride):
            left = line[x - channels] if x >= channels else 0
            up = prev[x]
            upleft = prev[x - channels] if x >= channels else 0
            if filter_type == 1:
                line[x] = (line[x] + left) & 255
            elif filter_type == 2:
                line[x] = (line[x] + up) & 255
            elif filter_type == 3:
                line[x] = (line[x] + ((left + up) >> 1)) & 255
            elif filter_type == 4:
                p = left + up - upleft
                pa, pb, pc = abs(p - left), abs(p - up), abs(p - upleft)
                pred = left if (pa <= pb and pa <= pc) else (up if pb <= pc else upleft)
                line[x] = (line[x] + pred) & 255
        out += line
        prev = line
    return width, height, color, bytes(out), palette


def pixel_at(color, buf, palette, index):
    if color == 3:
        j = buf[index] * 3
        return palette[j], palette[j + 1], palette[j + 2]
    stride = 3 if color == 2 else 4
    o = index * stride
    return buf[o], buf[o + 1], buf[o + 2]


def main(argv):
    step = 0
    top = 12
    paths = []
    i = 0
    while i < len(argv):
        if argv[i] == '--step':
            step = int(argv[i + 1])
            i += 2
        elif argv[i] == '--top':
            top = int(argv[i + 1])
            i += 2
        else:
            paths.append(argv[i])
            i += 1
    for path in paths:
        width, height, color, buf, palette = decode_png(path)
        total = width * height
        if step <= 0:
            step = max(2, int(total / 400000) ** 2 + 1)
        counts = Counter()
        sampled = 0
        for index in range(0, total, step):
            r, g, b = pixel_at(color, buf, palette, index)
            counts[(r & 0xF0, g & 0xF0, b & 0xF0)] += 1
            sampled += 1
        print('== %s  %dx%d  sampled=%d/%d (step=%d)' % (path, width, height, sampled, total, step))
        for (r, g, b), n in counts.most_common(top):
            print('   #%02x%02x%02x  %5.2f%%' % (r, g, b, 100.0 * n / sampled))


if __name__ == '__main__':
    main(sys.argv[1:])
