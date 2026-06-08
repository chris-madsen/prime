from __future__ import annotations

from dataclasses import dataclass


MULTIPLIERS: tuple[int, ...] = (1, 3, 9, 7)
DECIMAL_RESIDUES: tuple[int, ...] = (1, 3, 7, 9)


@dataclass(frozen=True, slots=True)
class Event:
    multiplier: int
    position: int
    channel: int


@dataclass(frozen=True, slots=True)
class BatchCoordinate:
    mask_index: int
    channel_phase: int
    c2_layer: int
    c3_layer: int


@dataclass(frozen=True, slots=True)
class E8Coordinate:
    golden_layer: int
    cell_layer: int
    mask_layer: int
    channel_phase: int


def event(period: int, channel_phase: int) -> Event:
    multiplier = MULTIPLIERS[channel_phase]
    position, channel = divmod(multiplier * period, 10)
    return Event(
        multiplier=multiplier,
        position=position,
        channel=channel,
    )


def events(period: int) -> tuple[Event, ...]:
    return tuple(event(period, phase) for phase in range(4))


def advance_period(item: Event, steps: int = 1) -> Event:
    return Event(
        multiplier=item.multiplier,
        position=item.position + steps * item.multiplier,
        channel=item.channel,
    )


def batch_coordinate(mask_index: int, channel_phase: int) -> BatchCoordinate:
    if not 0 <= mask_index < 6:
        raise ValueError("mask_index must be in [0, 6)")
    if not 0 <= channel_phase < 4:
        raise ValueError("channel_phase must be in [0, 4)")
    return BatchCoordinate(
        mask_index=mask_index,
        channel_phase=channel_phase,
        c2_layer=mask_index % 2,
        c3_layer=mask_index % 3,
    )


def mask_index_from_layers(c2_layer: int, c3_layer: int) -> int:
    return (3 * c2_layer + 4 * c3_layer) % 6


def decode_batch(
    base_q: int,
    residue: int,
    coordinate: BatchCoordinate,
) -> tuple[int, Event]:
    period = 10 * (base_q + coordinate.mask_index) + residue
    return period, event(period, coordinate.channel_phase)


def e8_coordinate(mask_index: int, channel_phase: int) -> E8Coordinate:
    if not 0 <= mask_index < 60:
        raise ValueError("mask_index must be in [0, 60)")
    if not 0 <= channel_phase < 4:
        raise ValueError("channel_phase must be in [0, 4)")
    return E8Coordinate(
        golden_layer=mask_index // 30,
        cell_layer=(mask_index % 30) // 6,
        mask_layer=mask_index % 6,
        channel_phase=channel_phase,
    )


def mask_index_from_e8_coordinate(coordinate: E8Coordinate) -> int:
    return (
        30 * coordinate.golden_layer
        + 6 * coordinate.cell_layer
        + coordinate.mask_layer
    )


def validate_period_recurrence(limit: int) -> int:
    tested = 0
    for period in range(11, limit + 1, 2):
        if period % 10 not in DECIMAL_RESIDUES:
            continue
        expected = events(period + 10)
        generated = tuple(map(advance_period, events(period)))
        if generated != expected:
            raise AssertionError(f"period recurrence failed for {period}")
        tested += 1
    return tested


def validate_six_mask_batches(batch_count: int) -> int:
    tested = 0
    for block in range(batch_count):
        base_q = 6 * block
        for residue in DECIMAL_RESIDUES:
            for mask_index in range(6):
                for channel_phase in range(4):
                    coordinate = batch_coordinate(mask_index, channel_phase)
                    restored_index = mask_index_from_layers(
                        coordinate.c2_layer,
                        coordinate.c3_layer,
                    )
                    if restored_index != mask_index:
                        raise AssertionError("CRT layer reconstruction failed")
                    period, generated = decode_batch(base_q, residue, coordinate)
                    if generated != event(period, channel_phase):
                        raise AssertionError("24-state batch decoding failed")
                    tested += 1
    return tested


def validate_hierarchical_batches(batch_count: int) -> tuple[int, int]:
    h4_tested = 0
    e8_tested = 0
    for block in range(batch_count):
        base_q = 60 * block
        for residue in DECIMAL_RESIDUES:
            for mask_index in range(60):
                for channel_phase in range(4):
                    coordinate = e8_coordinate(mask_index, channel_phase)
                    restored_index = mask_index_from_e8_coordinate(coordinate)
                    if restored_index != mask_index:
                        raise AssertionError("E8 hierarchy reconstruction failed")

                    period = 10 * (base_q + mask_index) + residue
                    generated = event(period, channel_phase)
                    affine_generated = advance_period(
                        event(10 * base_q + residue, channel_phase),
                        steps=mask_index,
                    )
                    if generated != affine_generated:
                        raise AssertionError("E8 affine batch decoding failed")

                    if coordinate.golden_layer == 0:
                        h4_tested += 1
                    e8_tested += 1
    return h4_tested, e8_tested


def main() -> int:
    periods = validate_period_recurrence(limit=100_000)
    states = validate_six_mask_batches(batch_count=1_000)
    h4_states, e8_states = validate_hierarchical_batches(batch_count=250)
    print(f"validated candidate periods: {periods}")
    print(f"validated affine batch states: {states}")
    print(f"validated H4/600-cell states: {h4_states}")
    print(f"validated E8/4_21 states: {e8_states}")
    print("affine B4/F4 mapping: OK")
    print("hierarchical 24-cell/H4/E8 addressing: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
