from __future__ import annotations

import argparse

from mask_math import describe_position


def main() -> None:
    parser = argparse.ArgumentParser(description="Показать, как строится один символ маски.")
    parser.add_argument("prime", type=int, nargs="?", default=13)
    parser.add_argument("k", type=int, nargs="?", default=1)
    args = parser.parse_args()

    info = describe_position(args.prime, args.k)
    print(f"p = {args.prime}, k = {args.k}")
    print(f"Кандидаты: {info.candidates}")
    if info.struck_digits:
        digits = ", ".join(str(digit) for digit in info.struck_digits)
        print(f"Выбиваются последние цифры: {digits}")
    else:
        print("На этой позиции prime p не выбивает ни один из четырёх кандидатов.")
    print(f"Символ маски: {info.symbol}")


if __name__ == "__main__":
    main()
