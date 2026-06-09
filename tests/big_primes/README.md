# Big prime test files

Each file contains a single decimal integer (no whitespace).
All numbers are proven primes from published sources (t5k.org).

## Primes

| File | Digits | Formula | Type | Source |
|------|--------|---------|------|--------|
| mersenne_2pow4423_minus1.txt | 1332 | 2^4423 − 1 | Mersenne prime #50 | t5k.org |
| mersenne_2pow9689_minus1.txt | 2917 | 2^9689 − 1 | Mersenne prime #52 | t5k.org |
| factorial_1477_plus1.txt | 4042 | 1477! + 1 | Factorial prime | t5k.org |
| factorial_1963_minus1.txt | 5614 | 1963! − 1 | Factorial prime | t5k.org |
| mersenne_2pow15823_minus1.txt | 4764 | 2^15823 − 1 | Mersenne prime #37 | t5k.org |
| mersenne_2pow19937_minus1.txt | 6002 | 2^19937 − 1 | Mersenne prime #38 | t5k.org |
| mersenne_2pow21701_minus1.txt | 6533 | 2^21701 − 1 | Mersenne prime #39 | t5k.org |
| mersenne_2pow23209_minus1.txt | 6987 | 2^23209 − 1 | Mersenne prime #40 | t5k.org |
| factorial_3507_minus1.txt | 10912 | 3507! − 1 | Factorial prime | t5k.org |
| factorial_3610_minus1.txt | 11277 | 3610! − 1 | Factorial prime | t5k.org |
| mersenne_2pow44497_minus1.txt | 13395 | 2^44497 − 1 | Mersenne prime #41 | t5k.org |
| factorial_6380_plus1.txt | 21507 | 6380! + 1 | Factorial prime | t5k.org |
| factorial_6917_minus1.txt | 23560 | 6917! − 1 | Factorial prime | t5k.org |
| mersenne_2pow86243_minus1.txt | 25962 | 2^86243 − 1 | Mersenne prime #42 | t5k.org |
| mersenne_2pow110503_minus1.txt | 33265 | 2^110503 − 1 | Mersenne prime #43 | t5k.org |

## Usage

    make isPrimeBigFile FILE=tests/big_primes/mersenne_2pow4423_minus1.txt
    make nextPrimeBigFile FILE=tests/big_primes/mersenne_2pow4423_minus1.txt BACKEND=cpu
