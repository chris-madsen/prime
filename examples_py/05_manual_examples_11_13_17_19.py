from __future__ import annotations

from mask_math import DIGITS, cleaned_mask, full_mask, self_hit_digit, self_hit_position


PRIMES = (11, 13, 17, 19)


def render_hits(p: int) -> None:
    print(f"\n=== p = {p} ===")
    for k in range(p):
        hits = [digit for digit in DIGITS if (10 * k + digit) % p == 0]
        if not hits:
            continue
        rendered = ", ".join(f"10*{k}+{digit}={10 * k + digit}" for digit in hits)
        print(f"k={k:2d}: {rendered}")
    print(f"G(p) = {full_mask(p)}")
    print(
        f"E(p) = {cleaned_mask(p)}  "
        f"(очистили позицию k={self_hit_position(p)} для числа {p}=10*{self_hit_position(p)}+{self_hit_digit(p)})"
    )


def main() -> None:
    print("Ручные контрольные примеры для p = 11, 13, 17, 19")
    for prime in PRIMES:
        render_hits(prime)


if __name__ == "__main__":
    main()
