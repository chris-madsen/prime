from __future__ import annotations

import argparse

from mask_math import cleaned_mask, full_mask, self_hit_digit, self_hit_position


def main() -> None:
    parser = argparse.ArgumentParser(description="Убрать из полной маски самовычеркивание числа p.")
    parser.add_argument("prime", type=int, nargs="?", default=13)
    args = parser.parse_args()

    p = args.prime
    base = full_mask(p)
    cleaned = cleaned_mask(p)
    k = self_hit_position(p)
    digit = self_hit_digit(p)
    print(f"p = {p} = 10*{k} + {digit}")
    print(f"Полная маска     : {base}")
    print(f"Позиция само-удара: k = {k}")
    print(f"На этой позиции число {10 * k + digit} = p, поэтому символ заменяется на 'f'.")
    print(f"Очищенная маска  : {cleaned}")


if __name__ == "__main__":
    main()
