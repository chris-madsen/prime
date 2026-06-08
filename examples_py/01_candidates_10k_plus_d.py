from __future__ import annotations

from mask_math import DIGITS, candidate_numbers


def main() -> None:
    print("Кандидаты имеют вид 10k + d, где d ∈ {1, 3, 7, 9}.\n")
    for k in range(12):
        candidates = candidate_numbers(k)
        rendered = ", ".join(f"10*{k}+{digit}={value}" for digit, value in zip(DIGITS, candidates))
        print(f"k={k:2d}: {rendered}")


if __name__ == "__main__":
    main()
