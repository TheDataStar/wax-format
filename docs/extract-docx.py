#!/usr/bin/env python3
"""Extract a .docx into greppable Markdown, preserving headings and tables.

    python docs/extract-docx.py docs/track-a-refinement.docx docs/track-a-refinement.md

Deliberately dependency-free (a .docx is a zip of XML) so regenerating the
plain-text copy never needs a toolchain that might not be installed. Verifies
that every source paragraph survives into the output and fails loudly if not.
"""

import re
import sys
import zipfile
from xml.etree import ElementTree as ET

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"


def para_text(p):
    return "".join(t.text or "" for t in p.iter(f"{W}t"))


def para_style(p):
    pPr = p.find(f"{W}pPr")
    if pPr is None:
        return ""
    st = pPr.find(f"{W}pStyle")
    return (st.get(f"{W}val") or "") if st is not None else ""


def is_list_item(p):
    pPr = p.find(f"{W}pPr")
    return pPr is not None and pPr.find(f"{W}numPr") is not None


def convert(src_path):
    root = ET.fromstring(zipfile.ZipFile(src_path).read("word/document.xml"))
    body = root.find(f"{W}body")
    out = []

    for el in body:
        if el.tag == f"{W}p":
            txt = para_text(el).strip()
            if not txt:
                out.append("")
                continue
            style = para_style(el)
            m = re.match(r"^Heading(\d)$", style)
            if m:
                out.append("#" * min(int(m.group(1)), 6) + " " + txt)
            elif style == "Title":
                out.append("# " + txt)
            elif is_list_item(el):
                out.append("- " + txt)
            else:
                out.append(txt)

        elif el.tag == f"{W}tbl":
            rows = []
            for tr in el.findall(f"{W}tr"):
                cells = []
                for tc in tr.findall(f"{W}tc"):
                    cell = " ".join(para_text(p).strip() for p in tc.findall(f"{W}p"))
                    cells.append(cell.strip().replace("|", r"\|"))
                rows.append(cells)
            if rows:
                width = max(len(r) for r in rows)
                rows = [r + [""] * (width - len(r)) for r in rows]
                out.append("")
                out.append("| " + " | ".join(rows[0]) + " |")
                out.append("|" + "|".join(["---"] * width) + "|")
                for r in rows[1:]:
                    out.append("| " + " | ".join(r) + " |")
                out.append("")

    text = re.sub(r"\n{3,}", "\n\n", "\n".join(out))
    return text.strip() + "\n", root


def verify(root, text):
    """Every non-empty source paragraph must survive into the output."""
    squashed = re.sub(r"\s+", "", text)
    missing = []
    for p in root.iter(f"{W}p"):
        raw = para_text(p)
        if re.sub(r"\s+", "", raw) and re.sub(r"\s+", "", raw) not in squashed:
            missing.append(raw[:100])
    return missing


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    src, dst = sys.argv[1], sys.argv[2]
    text, root = convert(src)
    missing = verify(root, text)
    if missing:
        print(f"ERROR: {len(missing)} source paragraph(s) lost in extraction:", file=sys.stderr)
        for m in missing[:10]:
            print("  " + m, file=sys.stderr)
        sys.exit(1)
    with open(dst, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
    print(f"{src} -> {dst} ({len(text)} chars, all paragraphs verified present)")


if __name__ == "__main__":
    main()
