"""Local workaround for Apple ld 27037 on macOS 27: it can leave the
LC_SYMTAB string pool 4-byte aligned when the indirect symbol count is odd,
which dyld rejects ("mis-aligned LINKEDIT string pool"). Shift the pool into
the padding before the code signature and re-sign ad hoc."""

import struct
import subprocess
import sys

path = sys.argv[1]
data = bytearray(open(path, "rb").read())
magic, _, _, _, ncmds, _, _, _ = struct.unpack_from("<IiiIIIII", data, 0)
assert magic == 0xFEEDFACF, "not a 64-bit Mach-O"
offset = 32
symtab = None
signature = None
for _ in range(ncmds):
    cmd, size = struct.unpack_from("<II", data, offset)
    if cmd == 0x2:
        symtab = offset
    if cmd == 0x1D:
        signature = struct.unpack_from("<II", data, offset + 8)[0]
    offset += size
symoff, nsyms, stroff, strsize = struct.unpack_from("<IIII", data, symtab + 8)
if stroff % 8 == 0:
    print("aligned already")
    sys.exit(0)
pad = 8 - stroff % 8
end = stroff + strsize
limit = signature if signature is not None else len(data)
assert end + pad <= limit, "no room to shift the string pool"
assert all(b == 0 for b in data[end : end + pad])
data[stroff + pad : end + pad] = data[stroff:end]
data[stroff : stroff + pad] = b"\0" * pad
struct.pack_into("<I", data, symtab + 8 + 8, stroff + pad)
open(path, "wb").write(data)
subprocess.run(["codesign", "-f", "-s", "-", path], check=True, capture_output=True)
print(f"shifted string pool by {pad}")
