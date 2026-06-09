# Масочный `isPrime(N)` и `nextPrime(N)` до 1024 десятичных знаков

## 0. Назначение документа

Этот документ фиксирует архитектурное решение для проекта `prime`:

- как проверять большое число `N` на простоту;
- как искать следующее простое после `N`;
- как использовать локальные маски без полного прохода от нуля;
- где маска является доказательством, а где только фильтром;
- как разделить быстрый режим и уточнённый доказательный режим;
- что именно надо реализовать в Rust для Claude.

Ограничение первой версии:

```text
N должен иметь не более 1024 десятичных знаков.
```

Это важное инженерное ограничение. Оно позволяет сделать рабочий production-слой для больших чисел, не уходя сразу в неуправляемые 10000+ знаков.

---

## 1. Главный вывод

Есть два разных режима:

```text
1. Быстрый практический режим:
   маска + Miller-Rabin

2. Строгий доказательный режим:
   маска + Miller-Rabin precheck + ECPP certificate
```

По умолчанию для `isPrime(N)` и `nextPrime(N)` должен запускаться **быстрый вариант**.

Под результатом должна быть кнопка:

```text
Уточнить ответ
```

При нажатии на неё запускается доказательный режим с ECPP.

---

## 2. Что такое маска в этой архитектуре

Маска — это оптимизированное локальное решето.

Она не обязана строиться от нуля до `N`.

Для каждого базового делителя `p` можно сразу вычислить его фазу в сегменте около `N`:

```text
offset = (-N) mod p
```

или в десятичной/wheel-форме проекта через соответствующий residue/channel state.

Это означает:

```text
Не нужно:
- считать все предыдущие маски;
- хранить все предыдущие маски;
- идти от 0 до N;
- докручивать состояние шаг за шагом.

Нужно:
- иметь базу делителей / seed базы;
- уметь локально вычислять offsets;
- строить сегментную маску около N.
```

---

## 3. Когда маска доказывает простоту

Маска действительно является доказательством простоты, если она полная.

Полная маска для кандидата `x` должна учитывать все возможные простые делители:

```text
p <= sqrt(x)
```

Потому что если `x` составное, то у него обязательно есть делитель не больше `sqrt(x)`.

Значит:

```text
Если все p <= sqrt(x) были учтены,
и x выжило,
то x простое.
```

Но для чисел на 1000 знаков полная база до корня практически невозможна:

```text
sqrt(10^1000) = 10^500
```

Поэтому в practical runtime маска используется не как полное доказательство, а как быстрый фильтр.

---

## 4. Когда маска является только фильтром

Если маска построена только по малым делителям, например до `10^6`, `10^7` или другого configurable limit, то результат означает только:

```text
N не делится на малые простые из базы.
```

Это не равно:

```text
N простое.
```

Составное число вида:

```text
N = q1 * q2
```

где оба множителя больше глубины базы, может выжить после малой маски.

Поэтому быстрый режим должен быть:

```text
малые маски + Miller-Rabin
```

А строгий режим:

```text
малые маски + Miller-Rabin + ECPP certificate
```

---

## 5. Почему Miller-Rabin нужен в быстром режиме

Miller-Rabin не ищет делители.

Он проверяет арифметическое свойство самого числа `N` через модульное возведение в степень.

Упрощённо один раунд выглядит так:

```text
pow_mod(a, d, N)
```

где `a` — основание теста.

Для 1000-значных чисел на Rust + GMP это практично и быстро.

Важно:

```text
Miller-Rabin даёт probable prime,
но не строгий certificate.
```

Для production UI результат нужно явно маркировать:

```text
Вероятно простое
```

или:

```text
Составное
```

Если Miller-Rabin нашёл свидетель составности, ответ `composite` точный.

Если Miller-Rabin сказал `probably prime`, это практический, но не доказательный ответ.

---

## 6. Зачем нужен ECPP

ECPP нужен для строгого ответа:

```text
Доказано простое
```

ECPP строит сертификат простоты, который можно потом быстро проверить.

Это не нейросетевой метод и не оптимизация гладкой функции.

В ECPP не нужны:

```text
sigmoid
нейросетевые активации
sin/cos как основная операция
обычная exp/log как главный runtime
```

Основные операции ECPP:

```text
bigint arithmetic
modular multiplication
modular exponentiation
gcd
elliptic curve arithmetic modulo N
point multiplication
partial factorization of group orders
certificate generation
certificate verification
```

Возможны вспомогательные высокоточные вычисления в некоторых CM/Atkin-Morain реализациях, но для нашего runtime это не похоже на ML/тригонометрический pipeline. Главная нагрузка — bigint и модульная арифметика.

---

## 7. Оценка времени для чисел около 1000 знаков

Условия оценки:

```text
CPU: i7, 6 ядер / 12 потоков
RAM: 64 GB
GPU: Nvidia GeForce GTX 1660 Ti
Backend для bigint: Rust + GMP
Размер N: около 1000 десятичных знаков
Ограничение первой версии: не более 1024 десятичных знаков
```

Таблица:

| Задача         | Метод                                  | Примерное время на 1000 знаков |
| -------------- | -------------------------------------- | -----------------------------: |
| `isPrime(N)`   | маска + Miller-Rabin 16–32             |               **0.05 – 1 сек** |
| `isPrime(N)`   | маска + Miller-Rabin 64                |                **0.2 – 3 сек** |
| `isPrime(N)`   | маска + ECPP                           |            **10 сек – 10 мин** |
| `nextPrime(N)` | маска + Miller-Rabin                   |                 **1 – 30 сек** |
| `nextPrime(N)` | маска + MR + ECPP финального кандидата |           **20 сек – 10+ мин** |

Эти оценки приблизительные. Реальное время зависит от:

```text
- реализации GMP bindings;
- числа Miller-Rabin rounds;
- глубины small-prime mask;
- размера сегмента для nextPrime;
- плотности выживших кандидатов;
- того, является ли N простым, составным с малым делителем или сложным composite;
- удачности ECPP certificate generation.
```

---

## 8. Роль GPU

GPU в текущей архитектуре полезен для массовой разметки сегментов:

```text
- marking multiples;
- updating segment bitsets;
- processing wheel residue classes;
- scanning E8/Sparse blocks;
- splitting CPU/GPU segments.
```

Но для одного конкретного `isPrime(N)` на 1000 знаков GPU почти не помогает.

Причина:

```text
Главная стоимость isPrime(N) после маски — bigint pow_mod.
Текущая CUDA/E8 архитектура ускоряет масочные блоки,
а не GMP modular exponentiation.
```

Для `nextPrime(N)` GPU может быть полезен, если строится достаточно большой локальный сегмент и надо массово просеять много кандидатов.

---

## 9. UI/UX поведение

### 9.1. `isPrime(N)` по умолчанию

При вводе числа пользователь нажимает:

```text
isPrime
```

Запускается быстрый режим:

```text
mask + Miller-Rabin
```

Возможные ответы:

```text
Составное
```

или:

```text
Вероятно простое
```

Под ответом обязательно показать кнопку:

```text
Уточнить ответ
```

Если пользователь нажимает кнопку, запускается:

```text
mask + Miller-Rabin precheck + ECPP certificate
```

Результат после уточнения:

```text
Доказано простое
```

или:

```text
Составное
```

---

### 9.2. `nextPrime(N)` по умолчанию

При вводе числа пользователь нажимает:

```text
nextPrime
```

Запускается быстрый режим:

```text
segment mask + Miller-Rabin on survivors
```

Результат:

```text
P = найденное вероятно простое число >= N
```

Под результатом обязательно показать кнопку:

```text
Уточнить ответ
```

При нажатии запускается ECPP только для финального кандидата `P`, а не для каждого survivor.

Правильный pipeline:

```text
1. segment mask
2. Miller-Rabin на survivors
3. выбрать первый probable prime P
4. по кнопке "Уточнить ответ":
   ECPP(P)
```

Неправильный pipeline:

```text
ECPP на каждый survivor
```

Так делать нельзя: это будет слишком дорого.

---

## 10. Прогресс вычислений

Если вычисление занимает дольше 3 секунд, UI обязан показывать прогресс в процентах.

### 10.1. Для Miller-Rabin

Тут прогресс считается точно:

```text
progress = completed_rounds / total_rounds * 100
```

Например:

```text
Miller-Rabin: 17 / 32 rounds = 53%
```

### 10.2. Для масочного сегмента

Прогресс считается по обработанным блокам:

```text
progress = processed_blocks / total_blocks * 100
```

или:

```text
progress = processed_segments / planned_segments * 100
```

### 10.3. Для `nextPrime(N)`

`nextPrime` заранее не знает, где будет следующий простое число.

Поэтому нужен staged progress:

```text
Stage 1: построение segment mask
Stage 2: scan survivors
Stage 3: Miller-Rabin candidates
Stage 4: расширение сегмента, если prime не найден
```

Проценты внутри текущего сегмента точные.

Глобальный процент до найденного простого — оценочный.

UI должен писать честно:

```text
Обработка текущего сегмента: 64%
Проверено кандидатов: 128
Найдено survivors: 9
Miller-Rabin: 5 / 32 rounds для текущего кандидата
```

### 10.4. Для ECPP

ECPP не всегда имеет заранее известное число шагов.

Поэтому процент может быть staged/heuristic:

```text
Stage 1: precheck
Stage 2: curve search
Stage 3: order computation / partial factorization
Stage 4: recursive certificate chain
Stage 5: certificate verification
```

Если нельзя честно посчитать общий процент, UI должен показывать:

```text
ECPP stage: curve search
elapsed: 00:01:37
attempts: 184
current subproblem digits: 742
```

Процент показывать только там, где он честный. Если процент эвристический, явно пометить:

```text
Оценочный прогресс: 42%
```

---

## 11. Ограничения первой версии

Для первой production-версии:

```text
max_decimal_digits = 1024
```

Если пользователь вводит больше:

```text
Ошибка:
Число слишком большое для текущей версии.
Максимум: 1024 десятичных знака.
```

Также нужно запретить:

```text
negative N для nextPrime
N с пробелами внутри
N с нецифровыми символами
N в scientific notation, если parser её не поддерживает
```

Допустимые форматы:

```text
123456789...
+123456789...  // можно нормализовать
```

---

## 12. Архитектура функций

### 12.1. Быстрый `isPrime`

```rust
pub enum FastPrimeResult {
    Composite { witness: CompositeWitness },
    ProbablePrime { rounds: u32 },
}

pub fn is_prime_fast(n: &BigInt, cfg: &PrimeQueryConfig) -> FastPrimeResult {
    // 1. Validate size <= 1024 decimal digits.
    // 2. Handle n < 2, n == 2/3/5/7.
    // 3. Wheel-210 quick reject.
    // 4. Small-prime mask / trial division.
    // 5. Miller-Rabin rounds.
}
```

### 12.2. Строгий `isPrime`

```rust
pub enum ProvenPrimeResult {
    Composite { witness: CompositeWitness },
    ProvenPrime { certificate: EcppCertificate },
}

pub fn is_prime_proven(n: &BigInt, cfg: &PrimeQueryConfig) -> ProvenPrimeResult {
    // 1. Run is_prime_fast.
    // 2. If Composite: return Composite.
    // 3. If ProbablePrime: run ECPP.
    // 4. Verify certificate before returning.
}
```

---

### 12.3. Быстрый `nextPrime`

```rust
pub struct NextPrimeFastResult {
    pub p: BigInt,
    pub offset: BigInt,
    pub checked_candidates: u64,
    pub mr_rounds: u32,
}

pub fn next_prime_fast(n: &BigInt, cfg: &PrimeQueryConfig) -> NextPrimeFastResult {
    // 1. Normalize start.
    // 2. Build local segment.
    // 3. Apply wheel-210.
    // 4. Apply small-prime masks using local offsets.
    // 5. Scan survivors.
    // 6. Run Miller-Rabin on survivors.
    // 7. Return first probable prime.
    // 8. If no candidate found, extend segment and repeat.
}
```

---

### 12.4. Строгий `nextPrime`

```rust
pub struct NextPrimeProvenResult {
    pub p: BigInt,
    pub offset: BigInt,
    pub checked_candidates: u64,
    pub certificate: EcppCertificate,
}

pub fn next_prime_proven(n: &BigInt, cfg: &PrimeQueryConfig) -> NextPrimeProvenResult {
    // 1. Run next_prime_fast.
    // 2. Take final candidate p.
    // 3. Run ECPP only for p.
    // 4. Verify certificate.
    // 5. Return proven result.
}
```

---

## 13. Configuration

```rust
pub struct PrimeQueryConfig {
    pub max_decimal_digits: usize,      // default 1024
    pub miller_rabin_rounds_fast: u32,  // default 32
    pub miller_rabin_rounds_strong: u32,// optional 64
    pub small_prime_limit: u64,         // configurable
    pub segment_blocks: usize,
    pub wheel: WheelKind,              // Wheel210
    pub backend: BackendKind,          // Cpu, Gpu, Hybrid
    pub progress_after_ms: u64,        // default 3000
    pub ecpp_enabled: bool,
}
```

Defaults:

```text
max_decimal_digits = 1024
miller_rabin_rounds_fast = 32
miller_rabin_rounds_strong = 64
progress_after_ms = 3000
wheel = 210
backend = cpu for isPrime
backend = cpu/hybrid for nextPrime depending on segment size
```

---

## 14. Progress API

Claude should implement a progress callback.

```rust
pub enum PrimeProgressStage {
    ValidateInput,
    WheelCheck,
    SmallMask,
    MillerRabin,
    BuildSegment,
    ScanSegment,
    ExtendSegment,
    EcppPrecheck,
    EcppCurveSearch,
    EcppFactorization,
    EcppCertificateChain,
    EcppVerify,
    Done,
}

pub struct PrimeProgress {
    pub stage: PrimeProgressStage,
    pub percent: Option<f64>,
    pub processed: Option<u64>,
    pub total: Option<u64>,
    pub message: String,
    pub elapsed_ms: u64,
    pub estimated: bool,
}
```

Rule:

```text
If elapsed time > 3000 ms:
    UI must display progress.
```

For exact progress:

```text
estimated = false
```

For ECPP/global nextPrime progress:

```text
estimated = true
```

unless the percentage is actually exact for the current stage.

---

## 15. UI requirements

### 15.1. `isPrime(N)`

Default button:

```text
Проверить
```

Runs:

```text
is_prime_fast
```

Output examples:

```text
Составное.
Найден делитель / свидетель составности.
```

or:

```text
Вероятно простое.
Метод: mask + Miller-Rabin 32 rounds.
```

Then show:

```text
[Уточнить ответ]
```

Click:

```text
is_prime_proven
```

Output:

```text
Доказано простое.
ECPP certificate generated and verified.
```

---

### 15.2. `nextPrime(N)`

Default button:

```text
Найти следующее простое
```

Runs:

```text
next_prime_fast
```

Output:

```text
Найдено вероятно простое P.
Offset: P - N.
Метод: segment mask + Miller-Rabin.
```

Then show:

```text
[Уточнить ответ]
```

Click:

```text
next_prime_proven
```

Output:

```text
Доказано простое P.
ECPP certificate generated and verified.
```

---

## 16. Implementation plan for Claude

### Phase 1 — input validation

Implement:

```rust
fn validate_decimal_input(s: &str) -> Result<BigInt, InputError>
```

Requirements:

```text
- trim outer spaces;
- allow optional leading '+';
- reject '-';
- reject internal spaces;
- reject non-digits;
- reject scientific notation unless explicitly supported;
- reject more than 1024 decimal digits;
- parse into GMP-backed BigInt.
```

---

### Phase 2 — GMP-backed bigint layer

Use Rust + GMP.

Acceptable options:

```text
- rug / gmp-mpfr-sys;
- direct GMP FFI wrapper;
- existing project-compatible bigint abstraction.
```

Needed operations:

```text
cmp
add/sub
mod
gcd
pow_mod
is_even
bit_length
decimal_digit_count
```

---

### Phase 3 — small-prime base

Implement small-prime table generation/cache.

```rust
pub struct SmallPrimeBase {
    pub limit: u64,
    pub primes: Vec<u64>,
}
```

Use:

```text
- fixed table for very small primes;
- sieve for configurable limit;
- cache by limit;
- reuse across requests.
```

---

### Phase 4 — wheel-210

Implement:

```rust
fn wheel210_reject(n: &BigInt) -> Option<SmallDivisor>
```

and candidate stepping for nextPrime:

```rust
fn next_wheel210_candidate_ge(n: &BigInt) -> BigInt
fn advance_wheel210_candidate(n: &mut BigInt)
```

Wheel-210 eliminates numbers divisible by:

```text
2, 3, 5, 7
```

---

### Phase 5 — local mask for one N

For `isPrime(N)`:

```rust
fn small_mask_check(n: &BigInt, base: &SmallPrimeBase) -> Option<u64>
```

It can be implemented as direct modular checks by small primes.

For one number this is fine.

For future optimization:

```text
batch residues
SIMD small primes
avoid repeated allocation
```

---

### Phase 6 — segment mask for nextPrime

Implement segment representation compatible with project storage modes:

```rust
enum SegmentStorage {
    E8Bitmap,
    SparseCsr,
    ImplicitFull,
}
```

Initial implementation can use a simple bitset first, then optimize.

Required logic:

```text
1. build candidate segment starting at N;
2. apply wheel-210;
3. for each small prime p:
   - compute start offset from N mod p;
   - clear multiples in segment;
4. scan survivors;
5. run Miller-Rabin on survivors;
6. extend segment if no probable prime found.
```

Do not create a temporary Vec of all events in hot loops.

Use callback/direct bit clearing.

---

### Phase 7 — Miller-Rabin

Implement:

```rust
fn miller_rabin(n: &BigInt, rounds: u32, progress: ProgressCallback)
    -> MillerRabinResult
```

Requirements:

```text
- handle small n;
- decompose n - 1 = d * 2^s;
- use deterministic small bases first;
- then configured bases if needed;
- report progress by completed rounds;
- stop early on composite witness;
- no unnecessary allocations in pow_mod loop.
```

For 1024 decimal digits:

```text
default rounds = 32
strong rounds = 64
```

---

### Phase 8 — fast isPrime

Implement:

```rust
pub fn is_prime_fast(
    n: &BigInt,
    cfg: &PrimeQueryConfig,
    progress: ProgressCallback,
) -> FastPrimeResult
```

Pipeline:

```text
validate
small constants
wheel-210
small-prime mask
Miller-Rabin
```

---

### Phase 9 — fast nextPrime

Implement:

```rust
pub fn next_prime_fast(
    n: &BigInt,
    cfg: &PrimeQueryConfig,
    progress: ProgressCallback,
) -> NextPrimeFastResult
```

Pipeline:

```text
validate
normalize start
loop:
    build segment
    apply masks
    scan survivors
    Miller-Rabin survivors
    if found: return
    extend segment
```

Important:

```text
- do not ECPP every survivor;
- ECPP only on final candidate in proven mode;
- progress must report segment index and candidate count.
```

---

### Phase 10 — ECPP integration

ECPP is a separate large subsystem.

First implement trait boundary:

```rust
pub trait PrimalityProver {
    fn prove_prime(
        &self,
        n: &BigInt,
        progress: ProgressCallback,
    ) -> Result<EcppCertificate, ProverError>;

    fn verify_certificate(
        &self,
        n: &BigInt,
        cert: &EcppCertificate,
    ) -> Result<bool, ProverError>;
}
```

Initial options:

```text
Option A:
    integrate existing ECPP library via FFI.

Option B:
    implement minimal ECPP later.

Option C:
    keep feature flag:
        --features ecpp
    and return "ECPP backend not configured" if missing.
```

Do not fake ECPP.

If ECPP is not implemented, UI must say:

```text
Уточнение недоступно: ECPP backend не подключён.
```

---

### Phase 11 — proven isPrime

Implement:

```rust
pub fn is_prime_proven(
    n: &BigInt,
    cfg: &PrimeQueryConfig,
    prover: &dyn PrimalityProver,
    progress: ProgressCallback,
) -> ProvenPrimeResult
```

Pipeline:

```text
1. Run is_prime_fast.
2. If composite: return composite.
3. If probable prime: run ECPP.
4. Verify certificate.
5. Return proven prime.
```

---

### Phase 12 — proven nextPrime

Implement:

```rust
pub fn next_prime_proven(
    n: &BigInt,
    cfg: &PrimeQueryConfig,
    prover: &dyn PrimalityProver,
    progress: ProgressCallback,
) -> NextPrimeProvenResult
```

Pipeline:

```text
1. Run next_prime_fast.
2. Take final probable prime P.
3. Run ECPP(P).
4. Verify certificate.
5. Return proven P.
```

---

### Phase 13 — CLI

Add commands:

```bash
make isPrime N=...
make isPrimeProven N=...
make nextPrime N=...
make nextPrimeProven N=...
```

CLI flags:

```text
--rounds 32
--strong-rounds 64
--small-prime-limit ...
--segment-blocks ...
--backend cpu|gpu|hybrid
--progress
--json
```

JSON output should include:

```json
{
  "input_digits": 1000,
  "method": "mask+miller-rabin",
  "status": "probable_prime",
  "rounds": 32,
  "elapsed_ms": 742,
  "can_refine": true
}
```

For proven:

```json
{
  "method": "mask+miller-rabin+ecpp",
  "status": "proven_prime",
  "elapsed_ms": 48123,
  "certificate_path": "..."
}
```

---

### Phase 14 — tests

Add tests:

```text
1. input length 1024 digits accepted;
2. input length 1025 digits rejected;
3. known small primes;
4. known composites;
5. Carmichael numbers;
6. strong pseudoprimes for several bases;
7. nextPrime small known values;
8. progress callback fires after 3 seconds in artificial slow mode;
9. "Уточнить ответ" path calls ECPP only once for final candidate;
10. ECPP missing backend returns clear error, not fake success.
```

---

### Phase 15 — benchmarks

Benchmarks to add:

```text
bench_isprime_100_digits
bench_isprime_1000_digits
bench_nextprime_1000_digits
bench_mr_round_1000_digits
bench_small_mask_limits
bench_segment_sizes
```

Record:

```text
- elapsed time;
- MR rounds;
- number of candidates scanned;
- number of survivors after mask;
- number of MR calls;
- ECPP time if available;
- backend cpu/gpu/hybrid.
```

---

## 17. Recommended default behavior

### `isPrime(N)`

Default:

```text
mask + Miller-Rabin 32
```

UI status:

```text
Вероятно простое
```

Refine button:

```text
Уточнить ответ
```

Refined:

```text
mask + Miller-Rabin + ECPP
```

UI status:

```text
Доказано простое
```

---

### `nextPrime(N)`

Default:

```text
segment mask + Miller-Rabin
```

UI status:

```text
Найдено вероятно простое
```

Refine button:

```text
Уточнить ответ
```

Refined:

```text
ECPP only for final candidate
```

UI status:

```text
Доказано простое
```

---

## 18. Final architecture summary

```text
For <=1024 decimal digits:

isPrimeFast:
    wheel-210
    small mask
    Miller-Rabin
    -> composite / probable prime

isPrimeProven:
    isPrimeFast
    ECPP only if probable prime
    -> composite / proven prime

nextPrimeFast:
    local segment mask near N
    scan survivors
    Miller-Rabin survivors
    -> first probable prime

nextPrimeProven:
    nextPrimeFast
    ECPP only for final candidate
    -> first proven prime
```

The key design rule:

```text
Do not compute previous masks.
Do not walk from 0 to N.
Use local offsets and segment masks.
Use Miller-Rabin for fast practical answers.
Use ECPP only when the user explicitly asks to refine.
```
