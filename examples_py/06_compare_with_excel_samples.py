from __future__ import annotations

import argparse
from pathlib import Path

from mask_math import cleaned_mask, full_mask, load_excel_mask_rows, special_symbols


def main() -> None:
    parser = argparse.ArgumentParser(description="Сверить построенные маски с Acset1.xlsx.")
    parser.add_argument("xlsx_path", nargs="?", default="Acset1.xlsx")
    args = parser.parse_args()

    path = Path(args.xlsx_path)
    rows = load_excel_mask_rows(path)
    mismatches: list[str] = []
    for row in rows:
        built_full = full_mask(row.prime)
        built_cleaned = cleaned_mask(row.prime)
        built_order = special_symbols(built_full)
        if built_full != row.full_mask:
            mismatches.append(f"row {row.row_number}: G mismatch for p={row.prime}")
        if built_cleaned != row.cleaned_mask:
            mismatches.append(f"row {row.row_number}: E mismatch for p={row.prime}")
        if built_order != row.symbol_order:
            mismatches.append(f"row {row.row_number}: H mismatch for p={row.prime}")

    print(f"Проверено строк: {len(rows)}")
    if mismatches:
        print(f"Найдено несовпадений: {len(mismatches)}")
        for line in mismatches[:20]:
            print(line)
        raise SystemExit(1)

    print("Все строки совпали:")
    print("- полная маска -> колонка G")
    print("- очищенная маска -> колонка E")
    print("- порядок специальных символов -> колонка H")


if __name__ == "__main__":
    main()
