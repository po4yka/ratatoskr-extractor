#!/usr/bin/env python3
"""Generate the deterministic synthetic PDF fixtures for direct extraction tests.

Stdlib only. Every fixture is first-party content written by this script; rerunning
it must reproduce byte-identical files. The encrypted fixtures implement the PDF
1.4 standard security handler revision 2 (RC4, 40-bit) exactly as specified.
"""

import hashlib
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent

PAD = bytes(
    [
        0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41,
        0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
        0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80,
        0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
    ]
)
FILE_ID = b"ratatoskr-fixture-id-01"
OWNER_PASSWORD = "ratatoskr-owner"
USER_PASSWORD = "ratatoskr-user"


def pad_password(password: bytes) -> bytes:
    return (password + PAD)[:32]


def rc4(key: bytes, data: bytes) -> bytes:
    state = list(range(256))
    mix = 0
    for index in range(256):
        mix = (mix + state[index] + key[index % len(key)]) % 256
        state[index], state[mix] = state[mix], state[index]
    out = bytearray()
    x = 0
    y = 0
    for byte in data:
        x = (x + 1) % 256
        y = (y + state[x]) % 256
        state[x], state[y] = state[y], state[x]
        out.append(byte ^ state[(state[x] + state[y]) % 256])
    return bytes(out)


def rc4_key(user_password: bytes, owner_entry: bytes, permissions: int) -> bytes:
    digest = hashlib.md5(
        pad_password(user_password)
        + owner_entry
        + (permissions & 0xFFFFFFFF).to_bytes(4, "little")
        + FILE_ID
    ).digest()
    return digest[:5]


def owner_entry(owner_password: bytes, user_password: bytes) -> bytes:
    return rc4(hashlib.md5(pad_password(owner_password)).digest()[:5], pad_password(user_password))


def user_entry_revision2(key: bytes) -> bytes:
    return rc4(key, PAD)


def object_key(key: bytes, objnum: int, gennum: int) -> bytes:
    extension = objnum.to_bytes(3, "little") + gennum.to_bytes(2, "little")
    return hashlib.md5(key + extension).digest()[: min(len(key) + 5, 16)]


class PdfBuilder:
    """Assembles numbered objects and a classic xref table with fixed offsets."""

    def __init__(self) -> None:
        self.objects: dict[int, bytes] = {}

    def add(self, number: int, body: bytes) -> None:
        self.objects[number] = body

    def encrypt(self, key: bytes) -> None:
        """RC4-encrypt every string and stream in place per object key."""
        for number, body in self.objects.items():
            self.objects[number] = encrypt_object_body(body, key, number)

    def render(self, info_number: int | None, encrypt_number: int | None) -> bytes:
        out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
        offsets: dict[int, int] = {}
        for number in sorted(self.objects):
            offsets[number] = len(out)
            out += f"{number} 0 obj\n".encode()
            out += self.objects[number]
            out += b"\nendobj\n"
        xref_at = len(out)
        count = max(self.objects) + 1
        out += f"xref\n0 {count}\n".encode()
        out += b"0000000000 65535 f \n"
        for number in range(1, count):
            if number in offsets:
                out += f"{offsets[number]:010d} 00000 n \n".encode()
            else:
                out += b"0000000000 65535 f \n"
        trailer = (
            b"trailer\n<< /Size " + str(count).encode() + b" /Root 1 0 R /Info "
            + str(info_number).encode() + b" 0 R"
        )
        if encrypt_number is not None:
            trailer += b" /Encrypt " + str(encrypt_number).encode() + b" 0 R"
        id_hex = FILE_ID.hex().encode()
        trailer += b" /ID [<" + id_hex + b"> <" + id_hex + b">] >>\n"
        out += trailer
        out += b"startxref\n" + str(xref_at).encode() + b"\n%%EOF\n"
        return bytes(out)


def pdf_string(raw: bytes) -> bytes:
    escaped = raw.replace(b"\\", b"\\\\").replace(b"(", br"\(").replace(b")", br"\)")
    return b"(" + escaped + b")"


def stream_object(dict_entries: bytes, data: bytes) -> bytes:
    return (
        b"<< " + dict_entries + b" /Length " + str(len(data)).encode() + b" >>\nstream\n"
        + data
        + b"\nendstream"
    )


def encrypt_object_body(body: bytes, key: bytes, number: int) -> bytes:
    objkey = object_key(key, number, 0)
    head, sep, rest = body.partition(b"stream")
    if sep and rest.startswith(b"\n"):
        # Encrypt only the stream data between "stream\n" and "\nendstream".
        data, end_sep, tail = rest[1:].rpartition(b"\nendstream")
        if end_sep:
            encrypted = rc4(objkey, data)
            return head + sep + b"\n" + encrypted + b"\nendstream" + tail
    # No stream: encrypt every literal string present at this nesting level.
    return encrypt_strings(head if not sep else body, objkey)


def encrypt_strings(body: bytes, objkey: bytes) -> bytes:
    out = bytearray()
    index = 0
    while index < len(body):
        byte = body[index : index + 1]
        if byte == b"(" and (index == 0 or body[index - 1 : index] != b"\\"):
            depth = 1
            cursor = index + 1
            while depth > 0:
                piece = body[cursor : cursor + 1]
                if piece == b"\\":
                    cursor += 2
                    continue
                if piece == b"(":
                    depth += 1
                elif piece == b")":
                    depth -= 1
                cursor += 1
            raw = unescape_pdf_string(body[index + 1 : cursor - 1])
            out += pdf_string(rc4(objkey, raw))
            index = cursor
        else:
            out += byte
            index += 1
    return bytes(out)


def unescape_pdf_string(raw: bytes) -> bytes:
    out = bytearray()
    index = 0
    while index < len(raw):
        byte = raw[index]
        if byte == 0x5C and index + 1 < len(raw):
            nxt = raw[index + 1]
            escapes = {ord("n"): 10, ord("r"): 13, ord("t"): 9, ord("b"): 8, ord("f"): 12}
            if nxt in escapes:
                out.append(escapes[nxt])
                index += 2
                continue
            if 0x30 <= nxt <= 0x37:
                digits = raw[index + 1 : index + 4]
                taken = 0
                value = 0
                for digit in digits:
                    if 0x30 <= digit <= 0x37 and taken < 3:
                        value = value * 8 + (digit - 0x30)
                        taken += 1
                    else:
                        break
                out.append(value)
                index += 1 + taken
                continue
            index += 2
            out.append(nxt)
            continue
        out.append(byte)
        index += 1
    return bytes(out)


FONT = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"


def text_stream(lines: list[tuple[float, float, str]], font_size: int = 12, leading: int = 18) -> bytes:
    parts = [b"BT", f"/F1 {font_size} Tf {leading} TL".encode()]
    previous_x: float | None = None
    for x, y, text in lines:
        encoded = text.encode("cp1252")
        if previous_x != x:
            parts.append(f"1 0 0 1 {x} {y} Tm".encode())
        else:
            parts.append(b"T*")
        parts.append(pdf_string(encoded) + b" Tj")
        previous_x = x
    parts.append(b"ET")
    return b"\n".join(parts)


def page_content(content_number: int) -> bytes:
    return (
        f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
        f"/Resources << /Font << /F1 4 0 R >> >> /Contents {content_number} 0 R >>"
    ).encode()


def build_simple_pdf(pages_lines: list[list[tuple[float, float, str]]]) -> PdfBuilder:
    builder = PdfBuilder()
    builder.add(1, b"<< /Type /Catalog /Pages 2 0 R >>")
    kids = " ".join(f"{5 + index * 2} 0 R" for index in range(len(pages_lines)))
    builder.add(2, f"<< /Type /Pages /Kids [{kids}] /Count {len(pages_lines)} >>".encode())
    builder.add(4, FONT)
    info_number = 3
    builder.add(3, b"<< /Title (Direct Extraction Fixture) /Producer (ratatoskr generate.py) >>")
    for index, lines in enumerate(pages_lines):
        page_number = 5 + index * 2
        content_number = page_number + 1
        builder.add(page_number, page_content(content_number))
        builder.add(content_number, stream_object(b"", text_stream(lines)))
    return builder


def write(name: str, data: bytes) -> None:
    target = HERE / name
    target.write_bytes(data)
    print(f"{name}: {len(data)} bytes sha256={hashlib.sha256(data).hexdigest()[:16]}")


def main() -> int:
    # Plain two-page text document; page order carries distinct sentences.
    plain = build_simple_pdf(
        [
            [(72, 720, "Ratatoskr direct extraction fixture."),
             (72, 702, "The first page carries deterministic prose for the parser."),
             (72, 684, "Un texte de contr\xf4le avec accents fran\xe7ais stables.")],
            [(72, 720, "Second page follows the first in the page tree."),
             (72, 702, "Reading order must keep this page after page one.")],
        ]
    )
    write("text-two-pages.pdf", plain.render(3, None))

    # Two-column single page: left column sentences then right column sentences.
    right_shifted = build_simple_pdf(
        [
            [(72, 720, "Left column opens the article body here."),
             (72, 702, "Left column continues with more deterministic prose."),
             (330, 720, "Right column starts its own narrative thread."),
             (330, 702, "Right column closes the two-column fixture body.")]
        ]
    )
    write("multi-column.pdf", right_shifted.render(3, None))

    # Encrypted requiring the user password (revision 2, RC4 40-bit).
    owner = owner_entry(OWNER_PASSWORD.encode(), USER_PASSWORD.encode())
    key = rc4_key(USER_PASSWORD.encode(), owner, -1)
    user = user_entry_revision2(key)
    encrypted = build_simple_pdf([[(72, 720, "Secret contents behind the standard security handler.")]])
    encrypted.add(
        99,
        (
            "<< /Filter /Standard /V 1 /R 2 /Length 40 "
            f"/O {_hex_string(owner)} /U {_hex_string(user)} /P -1 >>"
        ).encode(),
    )
    encrypted.encrypt(key)
    write("encrypted-user-password.pdf", encrypted.render(3, 99))

    # Encrypted with an empty user password: decrypt("") must succeed.
    blank_owner = owner_entry(OWNER_PASSWORD.encode(), b"")
    blank_key = rc4_key(b"", blank_owner, -1)
    blank_user = user_entry_revision2(blank_key)
    blank = build_simple_pdf([[(72, 720, "Readable after blank-password decryption.")]])
    blank.add(
        99,
        (
            "<< /Filter /Standard /V 1 /R 2 /Length 40 "
            f"/O {_hex_string(blank_owner)} /U {_hex_string(blank_user)} /P -1 >>"
        ).encode(),
    )
    blank.encrypt(blank_key)
    write("encrypted-blank-password.pdf", blank.render(3, 99))

    # Image-only style page: graphics operators only, no text operators.
    scanned = PdfBuilder()
    scanned.add(1, b"<< /Type /Catalog /Pages 2 0 R >>")
    scanned.add(2, b"<< /Type /Pages /Kids [5 0 R] /Count 1 >>")
    scanned.add(4, FONT)
    scanned.add(3, b"<< /Title (Scanned Fixture) /Producer (ratatoskr generate.py) >>")
    scanned.add(5, page_content(6))
    scanned.add(6, stream_object(b"", b"0 0 m 200 200 l S\n300 0 m 500 250 l S\n"))
    write("no-text-layer.pdf", scanned.render(3, None))

    # Oversized relative to test budgets: repeated valid lines inflate size.
    padded_lines = [(72, 720 - 14 * index, f"Padding line {index} repeats deterministic words.") for index in range(60)] * 12
    oversized = build_simple_pdf([[line for line in padded_lines if line[1] > 40]])
    write("oversized-padded.pdf", oversized.render(3, None))

    # Corrupt structure: valid header, truncated before the xref and trailer.
    whole = plain.render(3, None)
    cut = whole.index(b"/Count 2") + 10
    write("corrupt-truncated.pdf", whole[:cut])

    return 0


def _hex_string(value: bytes) -> str:
    return "<" + value.hex() + ">"


if __name__ == "__main__":
    sys.exit(main())
