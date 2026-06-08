from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from math import gcd
from pathlib import Path
from typing import Any

from mask_math import (
    SYMBOL_BY_DIGIT,
    cleaned_mask,
    full_mask,
    hit_positions_by_digit,
    load_excel_mask_rows,
    self_hit_digit,
    self_hit_position,
)


WHEEL = 840
CHANNEL_GENERATOR = 673
MIRROR = WHEEL - 1
MULTIPLIER_ORBIT: tuple[int, ...] = (1, 3, 9, 7)
MIRROR_MULTIPLIER: dict[int, int] = {1: 9, 9: 1, 3: 7, 7: 3}

U8_COORDINATES: dict[int, tuple[int, int]] = {
    (pow(7, a, 8) * pow(3, b, 8)) % 8: (a, b)
    for a in range(2)
    for b in range(2)
}
U7_COORDINATES: dict[int, tuple[int, int]] = {
    (pow(6, sign, 7) * pow(2, layer, 7)) % 7: (sign, layer)
    for sign in range(2)
    for layer in range(3)
}
U5_COORDINATES: dict[int, int] = {
    pow(3, channel, 5): channel
    for channel in range(4)
}


@dataclass(frozen=True, slots=True)
class WheelCoordinate:
    residue: int
    layer: int
    external_bit_1: int
    external_bit_2: int
    tesseract_index: int
    channel_phase: int
    internal_bit_1: int
    internal_bit_2: int
    vertex: int
    address: int
    word: int
    bit: int


def permute_vertex_bits(vertex: int) -> int:
    bit_0 = vertex & 0b0001
    bit_1 = vertex & 0b0010
    bit_2 = vertex & 0b0100
    bit_3 = vertex & 0b1000
    return bit_0 | bit_1 | (bit_2 << 1) | (bit_3 >> 1)


def galois_rotation(vertex: int) -> int:
    return permute_vertex_bits(vertex) ^ 0b0100


def apply_repeatedly(value: int, operation: Any, count: int) -> int:
    result = value
    for _ in range(count):
        result = operation(result)
    return result


def tesseract_vertex(channel_phase: int, internal_1: int, internal_2: int) -> int:
    rotated = apply_repeatedly(0, galois_rotation, channel_phase)
    translated_1 = rotated ^ (0b1101 if internal_1 else 0)
    return translated_1 ^ (0b1110 if internal_2 else 0)


def factor_coordinates(residue: int) -> tuple[int, int, int, int, int, int]:
    if not 0 <= residue < WHEEL or gcd(residue, WHEEL) != 1:
        raise ValueError(f"{residue} is not a unit modulo {WHEEL}")

    u8_neg, u8_three = U8_COORDINATES[residue % 8]
    u3_neg = 0 if residue % 3 == 1 else 1
    u7_neg, layer = U7_COORDINATES[residue % 7]
    channel_phase = U5_COORDINATES[residue % 5]

    internal_1 = u8_neg
    internal_2 = u8_three
    external_1 = u8_neg ^ u3_neg
    external_2 = u8_neg ^ u7_neg
    return layer, external_1, external_2, channel_phase, internal_1, internal_2


def wheel_coordinate(residue: int) -> WheelCoordinate:
    canonical = residue % WHEEL
    (
        layer,
        external_1,
        external_2,
        channel_phase,
        internal_1,
        internal_2,
    ) = factor_coordinates(canonical)
    tesseract_index = 4 * layer + 2 * external_1 + external_2
    vertex = tesseract_vertex(channel_phase, internal_1, internal_2)
    address = 16 * tesseract_index + vertex
    return WheelCoordinate(
        residue=canonical,
        layer=layer,
        external_bit_1=external_1,
        external_bit_2=external_2,
        tesseract_index=tesseract_index,
        channel_phase=channel_phase,
        internal_bit_1=internal_1,
        internal_bit_2=internal_2,
        vertex=vertex,
        address=address,
        word=address // 64,
        bit=address % 64,
    )


def serialize_coordinate(coordinate: WheelCoordinate) -> dict[str, Any]:
    return {
        "residue": coordinate.residue,
        "layer": coordinate.layer,
        "external_bits": (
            f"{coordinate.external_bit_1}{coordinate.external_bit_2}"
        ),
        "tesseract_index": coordinate.tesseract_index,
        "channel_phase": coordinate.channel_phase,
        "internal_bits": (
            f"{coordinate.internal_bit_1}{coordinate.internal_bit_2}"
        ),
        "vertex": coordinate.vertex,
        "vertex_bits": format(coordinate.vertex, "04b"),
        "address": coordinate.address,
        "word": coordinate.word,
        "bit": coordinate.bit,
    }


def raw_crt_residues(value: int) -> dict[str, int]:
    return {
        "mod_8": value % 8,
        "mod_3": value % 3,
        "mod_5": value % 5,
        "mod_7": value % 7,
    }


def mask_event(prime: int, multiplier: int) -> dict[str, Any]:
    value = multiplier * prime
    position, channel = divmod(value, 10)
    if channel not in SYMBOL_BY_DIGIT:
        raise ValueError(f"{value} is outside the decimal candidate channels")

    wheel_eligible = gcd(value, WHEEL) == 1
    return {
        "multiplier": multiplier,
        "value": value,
        "position": position,
        "channel": channel,
        "symbol": SYMBOL_BY_DIGIT[channel],
        "crt_residues": raw_crt_residues(value),
        "wheel_eligible": wheel_eligible,
        "wheel_coordinate": (
            serialize_coordinate(wheel_coordinate(value))
            if wheel_eligible
            else None
        ),
    }


def wheel_units() -> list[int]:
    return [residue for residue in range(WHEEL) if gcd(residue, WHEEL) == 1]


def wheel_gaps(units: list[int]) -> list[int]:
    extended = units[1:] + [units[0] + WHEEL]
    return [right - left for left, right in zip(units, extended)]


def build_lookup_tables() -> dict[str, Any]:
    units = wheel_units()
    coordinates = [wheel_coordinate(residue) for residue in units]
    by_address = sorted(coordinates, key=lambda coordinate: coordinate.address)

    if len({coordinate.address for coordinate in coordinates}) != 192:
        raise ValueError("wheel coordinates are not bijective")

    return {
        "numeric_residues": units,
        "numeric_gaps": wheel_gaps(units),
        "residue_to_bit": [
            serialize_coordinate(coordinate)
            for coordinate in coordinates
        ],
        "bit_to_residue": [
            {
                "address": coordinate.address,
                "word": coordinate.word,
                "bit": coordinate.bit,
                "residue": coordinate.residue,
            }
            for coordinate in by_address
        ],
    }


def validate_symmetries() -> None:
    for residue in wheel_units():
        coordinate = wheel_coordinate(residue)
        rotated = wheel_coordinate(CHANNEL_GENERATOR * residue)
        mirrored = wheel_coordinate(MIRROR * residue)

        if rotated.tesseract_index != coordinate.tesseract_index:
            raise ValueError("channel generator changed the tesseract index")
        if rotated.channel_phase != (coordinate.channel_phase + 1) % 4:
            raise ValueError("channel generator did not advance C4")
        if rotated.vertex != galois_rotation(coordinate.vertex):
            raise ValueError("channel generator did not apply the vertex rotation")

        if mirrored.tesseract_index != coordinate.tesseract_index:
            raise ValueError("mirror changed the tesseract index")
        if mirrored.vertex != (coordinate.vertex ^ 0b0001):
            raise ValueError("mirror is not the selected tesseract edge")


def validate_mask_events(prime: int, events: list[dict[str, Any]]) -> None:
    positions = hit_positions_by_digit(prime)
    for event in events:
        channel = int(event["channel"])
        if positions[channel] != event["position"]:
            raise ValueError(f"p={prime}: multiplier event does not match G")

    by_multiplier = {
        int(event["multiplier"]): event
        for event in events
    }
    for left, right in ((1, 9), (3, 7)):
        left_event = by_multiplier[left]
        right_event = by_multiplier[right]
        if left_event["position"] + right_event["position"] != prime - 1:
            raise ValueError(f"p={prime}: mirror positions do not match")
        if left_event["channel"] + right_event["channel"] != 10:
            raise ValueError(f"p={prime}: mirror channels do not match")


def sieve_seed(prime: int, units: list[int]) -> dict[str, Any]:
    first_composite = prime * prime
    quotient_residue = prime % WHEEL
    quotient_phase = units.index(quotient_residue)
    composite_coordinate = wheel_coordinate(first_composite)
    return {
        "first_composite": first_composite,
        "block_index": first_composite // WHEEL,
        "residue": first_composite % WHEEL,
        "coordinate": serialize_coordinate(composite_coordinate),
        "quotient_residue": quotient_residue,
        "wheel_phase": quotient_phase,
        "advance_rule": [
            "composite += prime * numeric_gaps[wheel_phase]",
            "wheel_phase = (wheel_phase + 1) mod 192",
        ],
    }


def build_record(row: Any, units: list[int]) -> dict[str, Any]:
    generated_full = full_mask(row.prime)
    generated_cleaned = cleaned_mask(row.prime)
    if generated_full != row.full_mask:
        raise ValueError(f"p={row.prime}: full mask differs from column G")
    if generated_cleaned != row.cleaned_mask:
        raise ValueError(f"p={row.prime}: cleaned mask differs from column E")

    events = [
        mask_event(row.prime, multiplier)
        for multiplier in MULTIPLIER_ORBIT
    ]
    validate_mask_events(row.prime, events)

    event_by_multiplier = {
        str(event["multiplier"]): event
        for event in events
    }
    return {
        "excel_row": row.row_number,
        "prime": row.prime,
        "full_mask_G": row.full_mask,
        "cleaned_mask_E": row.cleaned_mask,
        "symbol_order_H": row.symbol_order,
        "diagnostic_orbit": {
            "order": list(MULTIPLIER_ORBIT),
            "events": events,
            "mirror_pairs": [
                {
                    "multipliers": [left, right],
                    "positions": [
                        event_by_multiplier[str(left)]["position"],
                        event_by_multiplier[str(right)]["position"],
                    ],
                }
                for left, right in ((1, 9), (3, 7))
            ],
            "self_hit": {
                "position": self_hit_position(row.prime),
                "channel": self_hit_digit(row.prime),
                "removed_from_E": True,
            },
        },
        "sieve_seed": sieve_seed(row.prime, units),
    }


def build_document(xlsx_path: Path) -> dict[str, Any]:
    validate_symmetries()
    rows = load_excel_mask_rows(xlsx_path)
    units = wheel_units()
    lookup_tables = build_lookup_tables()
    return {
        "schema": "acset1-tesseract-wheel-research-v2",
        "source": {
            "workbook": xlsx_path.name,
            "sheet": "E",
            "full_mask_column": "G",
            "cleaned_mask_column": "E",
            "symbol_order_column": "H",
        },
        "model": {
            "wheel": WHEEL,
            "unit_count": 192,
            "group_decomposition": "U(840) ~= C3 x C2^4 x C4",
            "strict_tesseract_decomposition": (
                "U(840) ~= (C3 x C2^2) x (C4 x C2^2)"
            ),
            "storage": {
                "tesseract_count": 12,
                "bits_per_tesseract": 16,
                "uint64_words": 3,
            },
            "factor_basis": {
                "mod_8": "residue = 7^a * 3^b; a,b in C2",
                "mod_3": "residue = (-1)^c; c in C2",
                "mod_7": "residue = (-1)^s * 2^layer; s in C2, layer in C3",
                "mod_5": "residue = 3^channel_phase; channel_phase in C4",
                "internal_1": "a",
                "internal_2": "b",
                "external_1": "a XOR c",
                "external_2": "a XOR s",
                "tesseract_index": "4*layer + 2*external_1 + external_2",
            },
            "channel_generator": {
                "residue": CHANNEL_GENERATOR,
                "crt": raw_crt_residues(CHANNEL_GENERATOR),
                "effect": "same tesseract, channel_phase + 1, apply R(vertex)",
            },
            "mirror": {
                "residue": MIRROR,
                "crt": raw_crt_residues(MIRROR),
                "effect": "same tesseract, vertex XOR 0001",
            },
            "vertex_operations": {
                "R": "swap vertex bits 2 and 3, then XOR 0100",
                "S": "XOR 1101",
                "T": "XOR 1110",
                "vertex_formula": "R^channel_phase S^internal_1 T^internal_2(0000)",
                "mirror_identity": "R^2 S = XOR 0001",
            },
            "runtime_note": (
                "G/E events are decimal-mask diagnostics. A wheel-840 sieve "
                "starts each prime at p^2 and advances through numeric_gaps."
            ),
        },
        "lookup_tables": lookup_tables,
        "record_count": len(rows),
        "records": [
            build_record(row, units)
            for row in rows
        ],
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Export Acset1 masks and the 840-wheel tesseract research model."
    )
    parser.add_argument("xlsx", nargs="?", default="Acset1.xlsx")
    parser.add_argument(
        "output",
        nargs="?",
        default="data/acset1_tesseract_masks.json",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    source = Path(args.xlsx)
    destination = Path(args.output)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(
        json.dumps(build_document(source), indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    print(f"Wrote {destination}")


if __name__ == "__main__":
    main()
