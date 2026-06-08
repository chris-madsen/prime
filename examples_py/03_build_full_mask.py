from __future__ import annotations

import argparse

from mask_math import full_mask, hit_positions_by_digit, special_symbols


def main() -> None:
    parser = argparse.ArgumentParser(description="Построить полную маску для простого p.")
    parser.add_argument("prime", type=int, nargs="?", default=13)
    args = parser.parse_args()

    p = args.prime
    mask = full_mask(p)
    positions = hit_positions_by_digit(p)
    print(f"p = {p}")
    print(f"Полная маска (соответствует колонке G): {mask}")
    print(f"Порядок специальных символов: {special_symbols(mask)}")
    print("Позиции ударов по последним цифрам:")
    for digit in (1, 3, 7, 9):
        print(f"  ...{digit} -> k = {positions[digit]}")


if __name__ == "__main__":
    main()
