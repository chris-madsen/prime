from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Iterable
from zipfile import ZipFile
import xml.etree.ElementTree as ET

DIGITS: tuple[int, ...] = (1, 3, 7, 9)
BIT_BY_DIGIT: dict[int, int] = {
    1: 0b1000,
    3: 0b0100,
    7: 0b0010,
    9: 0b0001,
}
HEX_BY_MASK: dict[int, str] = {value: format(value, "x") for value in range(16)}
SYMBOL_BY_DIGIT: dict[int, str] = {
    1: "7",
    3: "b",
    7: "d",
    9: "e",
}
DIGIT_BY_SYMBOL: dict[str, int] = {value: key for key, value in SYMBOL_BY_DIGIT.items()}
NS = {"a": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}


@dataclass(frozen=True, slots=True)
class MaskPosition:
    k: int
    candidates: tuple[int, int, int, int]
    struck_digits: tuple[int, ...]
    symbol: str


@dataclass(frozen=True, slots=True)
class ExcelMaskRow:
    row_number: int
    prime: int
    cleaned_mask: str
    full_mask: str
    symbol_order: str


def candidate_numbers(k: int) -> tuple[int, int, int, int]:
    return tuple(10 * k + digit for digit in DIGITS)


def struck_digits_for_position(p: int, k: int) -> tuple[int, ...]:
    return tuple(digit for digit in DIGITS if (10 * k + digit) % p == 0)


def symbol_for_position(p: int, k: int) -> str:
    mask = 0b1111
    for digit in struck_digits_for_position(p, k):
        mask &= ~BIT_BY_DIGIT[digit]
    return HEX_BY_MASK[mask]


def describe_position(p: int, k: int) -> MaskPosition:
    return MaskPosition(
        k=k,
        candidates=candidate_numbers(k),
        struck_digits=struck_digits_for_position(p, k),
        symbol=symbol_for_position(p, k),
    )


def full_mask(p: int) -> str:
    return "".join(symbol_for_position(p, k) for k in range(p))


def self_hit_position(p: int) -> int:
    q, r = divmod(p, 10)
    if r not in DIGITS:
        raise ValueError(f"p={p} is not of the form 10q + 1/3/7/9")
    return q


def self_hit_digit(p: int) -> int:
    _, r = divmod(p, 10)
    if r not in DIGITS:
        raise ValueError(f"p={p} is not of the form 10q + 1/3/7/9")
    return r


def cleaned_mask(p: int) -> str:
    base = list(full_mask(p))
    base[self_hit_position(p)] = "f"
    return "".join(base)


def special_symbols(mask: str) -> str:
    return "".join(symbol for symbol in mask if symbol != "f")


def inverse_mod_10(p: int) -> int:
    return pow(10, -1, p)


def hit_positions_by_digit(p: int) -> dict[int, int]:
    inv10 = inverse_mod_10(p)
    return {digit: (-digit * inv10) % p for digit in DIGITS}


def load_shared_strings(zf: ZipFile) -> list[str]:
    if "xl/sharedStrings.xml" not in zf.namelist():
        return []
    root = ET.fromstring(zf.read("xl/sharedStrings.xml"))
    out: list[str] = []
    for si in root.findall("a:si", NS):
        t = si.find("a:t", NS)
        if t is not None:
            out.append(t.text or "")
            continue
        parts: list[str] = []
        for run in si.findall("a:r", NS):
            tt = run.find("a:t", NS)
            if tt is not None:
                parts.append(tt.text or "")
        out.append("".join(parts))
    return out


def load_sheet_rows(xlsx_path: Path, sheet_path: str) -> list[dict[str, str]]:
    with ZipFile(xlsx_path) as zf:
        sst = load_shared_strings(zf)
        root = ET.fromstring(zf.read(sheet_path))
    rows: list[dict[str, str]] = []
    for row in root.find("a:sheetData", NS).findall("a:row", NS):
        cells: dict[str, str] = {}
        for cell in row.findall("a:c", NS):
            ref = cell.attrib["r"]
            cell_type = cell.attrib.get("t")
            value_node = cell.find("a:v", NS)
            inline_node = cell.find("a:is", NS)
            value = ""
            if cell_type == "s" and value_node is not None:
                value = sst[int(value_node.text)]
            elif cell_type == "inlineStr" and inline_node is not None:
                text_node = inline_node.find("a:t", NS)
                value = text_node.text if text_node is not None else ""
            elif value_node is not None and value_node.text is not None:
                value = value_node.text
            cells[ref] = value
        rows.append(cells)
    return rows


def load_excel_mask_rows(xlsx_path: Path) -> list[ExcelMaskRow]:
    rows = load_sheet_rows(xlsx_path, "xl/worksheets/sheet5.xml")
    out: list[ExcelMaskRow] = []
    for row_number, row in enumerate(rows, start=1):
        if row_number < 6:
            continue
        prime_text = row.get(f"F{row_number}", "")
        if not prime_text:
            continue
        out.append(
            ExcelMaskRow(
                row_number=row_number,
                prime=int(prime_text),
                cleaned_mask=row[f"E{row_number}"],
                full_mask=row[f"G{row_number}"],
                symbol_order=row[f"H{row_number}"],
            )
        )
    return out


def take(items: Iterable[object], size: int) -> list[object]:
    out: list[object] = []
    for item in items:
        out.append(item)
        if len(out) == size:
            break
    return out
