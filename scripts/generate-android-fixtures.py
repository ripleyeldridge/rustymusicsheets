#!/usr/bin/env python3
"""Generate original score fixtures; never reads a personal music library."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import zlib


def pdf(path, pages):
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        ("<< /Type /Pages /Kids [" + " ".join(f"{4 + i * 2} 0 R" for i in range(len(pages))) + f"] /Count {len(pages)} >>").encode(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    for index, labels in enumerate(pages):
        stream = "".join(
            f"BT /F1 28 Tf 50 {740 - row * 45} Td ({label}) Tj ET\n"
            for row, label in enumerate(labels)
        )
        stream += "1 0 0 rg 70 340 120 140 re f\n0 1 0 rg 240 340 120 140 re f\n0 0 1 rg 410 340 120 140 re f\n"
        objects += [
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {5 + index * 2} 0 R >>".encode(),
            f"<< /Length {len(stream)} >>\nstream\n{stream}\nendstream".encode(),
        ]
    write_pdf(path, objects)


def write_pdf(path, objects):
    data = bytearray(b"%PDF-1.4\n")
    offsets = [0]
    for index, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(f"{index} 0 obj\n".encode() + obj + b"\nendobj\n")
    xref = len(data)
    data.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        data.extend(f"{offset:010} 00000 n \n".encode())
    data.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    path.write_bytes(data)


def scanned_pdf(path, jpeg, width, height):
    stream = b"q 612 0 0 792 0 0 cm /Scan Do Q"
    image = f"<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {len(jpeg)} >>\nstream\n".encode() + jpeg + b"\nendstream"
    write_pdf(path, [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Scan 5 0 R >> >> /Contents 4 0 R >>",
        f"<< /Length {len(stream)} >>\nstream\n".encode() + stream + b"\nendstream",
        image,
    ])


def png_chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output
    output.mkdir(parents=True, exist_ok=True)
    marker = output / "fixtures.json"
    if any(output.iterdir()) and (not marker.exists() or json.loads(marker.read_text()).get("generator") != "rustymusicsheets-original-fixtures-v1"):
        parser.error("Output must be empty or an existing generated fixture directory")
    if not shutil.which("pdftoppm"):
        parser.error("Poppler pdftoppm is required to generate image fixtures")
    pdf(output / "text-score.pdf", [["Cello", "Piano", "Alto Saxophone 1"]])
    pdf(output / "multipage.pdf", [["Cello"], ["Piano"], ["Violin II"]])
    for option, suffix in [("-png", "png"), ("-jpeg", "jpg")]:
        subprocess.run(["pdftoppm", "-singlefile", "-scale-to", "1200", option, str(output / "text-score.pdf"), str(output / "score")], check=True)
        assert (output / f"score.{suffix}").is_file()
    # Same visible page, distinct encoded content for a real Unicode-title import.
    png = (output / "score.png").read_bytes()
    (output / "Étude Unicode.png").write_bytes(png[:-12] + png_chunk(b"tEXt", "Title\0Étude".encode("latin-1")) + png[-12:])
    width, height = struct.unpack(">II", (output / "score.png").read_bytes()[16:24])
    scanned_pdf(output / "scanned.pdf", (output / "score.jpg").read_bytes(), width, height)
    (output / "corrupt.png").write_bytes(b"Not an image: original malformed fixture\n")
    header = struct.pack(">IIBBBBB", 10000, 5000, 8, 2, 0, 0, 0)
    (output / "oversized.png").write_bytes(b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", header) + png_chunk(b"IDAT", zlib.compress(b"")) + png_chunk(b"IEND", b""))
    entries = []
    for name in ("text-score.pdf", "multipage.pdf", "score.png", "score.jpg", "Étude Unicode.png", "scanned.pdf", "corrupt.png", "oversized.png"):
        path = output / name
        mime = "application/pdf" if path.suffix == ".pdf" else "image/jpeg" if path.suffix == ".jpg" else "image/png"
        entries.append({"name": name, "mime": mime, "bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "valid": name not in ("corrupt.png", "oversized.png")})
    marker.write_text(json.dumps({"generator": "rustymusicsheets-original-fixtures-v1", "files": entries}, indent=2, ensure_ascii=False) + "\n")
    print(f"Generated {len(entries)} original score fixtures in {output}")


if __name__ == "__main__":
    main()
