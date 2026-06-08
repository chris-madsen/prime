# Практическая оптимизация \(E_8\)/24-cell кодека на Rust

## Итоговый формат

Хранить данные следует крупными \(E_8\)-блоками:

```rust
#[repr(C, align(32))]
pub struct E8MaskBlock {
    selectors: [u64; 4],
}
```

Каждый `u64` содержит 60 значащих бит одного десятичного класса. Весь блок
занимает:

$$
4\cdot8=32\text{ байта}.
$$

Логически каждый selector состоит из десяти 24-cell подблоков:

$$
60=10\cdot6.
$$

Физически разбивать его на десять байтов не нужно.

## Sparse-путь

```rust
pub fn scan_sparse(
    mut selector: u64,
    base_p: u64,
    mut emit: impl FnMut([u64; 4]),
) {
    selector &= (1_u64 << 60) - 1;

    while selector != 0 {
        let n = selector.trailing_zeros() as u64;
        selector &= selector - 1;

        let p = base_p + 10 * n;
        emit([p, 3 * p, 9 * p, 7 * p]);
    }
}
```

Он обрабатывает только установленные биты.

## Dense-путь

Для очень плотного сегмента используется последовательный проход:

```rust
pub fn scan_dense(
    selector: u64,
    base_p: u64,
    mut emit: impl FnMut([u64; 4]),
) {
    let selector = selector & ((1_u64 << 60) - 1);
    let mut p = base_p;

    for n in 0..60 {
        if selector & (1_u64 << n) != 0 {
            emit([p, 3 * p, 9 * p, 7 * p]);
        }
        p += 10;
    }
}
```

На текущем x86-64 процессоре sparse-путь быстрее примерно до 50% заполнения.
Режим выбирается один раз для целого сегмента:

```rust
pub enum ScanMode {
    Sparse,
    Dense,
}

pub const fn choose_scan_mode(active: u64, slots: u64) -> ScanMode {
    if active.saturating_mul(2) <= slots {
        ScanMode::Sparse
    } else {
        ScanMode::Dense
    }
}
```

Порог зависит от CPU и уточняется benchmark.

## Проверка 24-cell LUT

Была реализована таблица всех:

$$
2^6=64
$$

6-битных паттернов.

На текущем CPU она проиграла прямому `trailing_zeros` при всех измеренных
плотностях. Причины:

- дополнительная загрузка таблицы;
- цикл по десяти подблокам;
- преобразование локального индекса в глобальный;
- аппаратный поиск младшего установленного бита уже достаточно дешёв.

Поэтому LUT оставлена только как исследовательский режим.

## CUDA и совместная работа CPU + GPU

CUDA-архитектура взята из проекта:

`/media/ilja/DATA/soft/fastsearch`.

Использованы его принципы:

- Rust как управляющий слой;
- CUDA kernel через `nvcc` и `build.rs`;
- крупные пакеты;
- постоянные контексты и streams;
- pinned double buffering;
- отсутствие передачи отдельных событий через PCIe.

На GTX 1660 Ti сравнивались:

- `Sparse`: один thread на `u64`;
- `DenseWarp`: один warp на `u64`;
- `Cell24Lookup`: LUT в constant memory.

`Sparse` победил от 5% до 95%. LUT стал быстрее только около 99–100%.
`DenseWarp` в decoder benchmark не победил.

При 100% selector хранить не нужно:

```rust
pub enum BlockStorage {
    ImplicitFull,
    Bitmap(E8MaskBlock),
    SparseCsr,
}
```

Поэтому основной GPU decoder — `Sparse`, а LUT остаётся экспериментальным.

Три прогретых запуска hybrid benchmark после внедрения постоянного CUDA pool:

- 4 194 304 selector-слова;
- плотность 25%;
- 30% блоков CPU;
- 70% блоков GPU;
- ускорение относительно CPU-only на 8 потоках: \(2{,}8\text{–}3{,}2\times\);
- режим уже steady-state: pinned-буферы и device-буферы переиспользуются.

Текущий основной путь для таких измерений — device-resident режим без длинной
обратной выгрузки массива событий через PCIe.

## Counting runtime

Добавлен отдельный checkpointed runner:

```bash
cargo run --release --bin count_first_n -- --n 1000000 --segment-size 1024
```

Он считает именно **первые** $n$ простых, а не все простые $\le n$, и по
умолчанию работает в streaming/checkpoint режиме:

- `found_count`;
- `last_prime`;
- `processed_segments`;
- статистика по выбранным storage mode.

Smoke-результаты текущего состояния:

- $p_{1000}=7919$;
- $p_{100000}=1299709$;
- $p_{10^6}=15485863$.

Контрольная инженерная цель следующего масштаба:

$$
p_{10^{12}} = 29\,996\,224\,275\,833.
$$


### Durable state и background-run

Текущий CPU runtime уже умеет:

- писать checkpoint атомарно через временный файл и `rename`;
- обновлять отдельный progress-файл для наблюдения;
- продолжать run через `--resume`;
- запрещать автопродолжение через `--no-resume`;
- сохраняться и по числу сегментов, и по таймеру;
- корректно завершаться по `SIGINT` и `SIGTERM` с записью последнего checkpoint.

Поддерживаемые CLI-флаги:

- `--checkpoint <path>`;
- `--progress <path>`;
- `--checkpoint-every <segments>`;
- `--checkpoint-interval-sec <sec>`;
- `--progress-interval-sec <sec>`;
- `--threads <n>`;
- `--archive-dir <path>`;
- `--resume`;
- `--no-resume`.

Типовой long run:

```bash
cargo run --release --bin count_first_n -- \
  --n 100000000000 \
  --backend cpu \
  --threads 8 \
  --segment-size 4096 \
  --checkpoint data/count_1e11.checkpoint \
  --progress data/count_1e11.progress \
  --checkpoint-every 64 \
  --checkpoint-interval-sec 60 \
  --progress-interval-sec 15
```

Продолжение после остановки:

```bash
cargo run --release --bin count_first_n -- \
  --n 100000000000 \
  --backend cpu \
  --threads 8 \
  --checkpoint data/count_1e11.checkpoint \
  --progress data/count_1e11.progress \
  --resume
```

Важно: текущий counting runtime теперь умеет исполняться и с `backend=gpu`, и с
`backend=hybrid`, то есть эти режимы больше не являются чисто декоративными
флагами. Архивный on-disk поток `--archive-dir` теперь использует delta-coded chunk-архив
с `manifest.txt` и `index.tsv`; на resume рантайм сначала reconcile-ит уже
закоммиченные chunk-файлы, а потом продолжает запись без дублирования.

Длинные рассуждения о practical roadmap, диапазоне $10^{19}\to10^{20}$ и
целевых интерфейсах `isPrime(N)` / `nextPrimeFrom(N)` вынесены в
`docs/tesseract_encode_ideas.md`.

## Память

На 240 периодов:

| Формат | Размер |
|---|---:|
| `ImplicitFull` | 0 байт |
| ideal bit-packed | 30 байт |
| `E8Bitmap` | 32 байта |
| byte-aligned 24-cell | 40 байт |

Для sparse CSR:

$$
B_{\mathrm{CSR}}=4(B+1)+A.
$$

Он выигрывает у bitmap приблизительно ниже 11,7% плотности.

При CPU+GPU обработке нужно разделять блоки, а не дублировать весь набор:

```text
all blocks = CPU-owned blocks ⊔ GPU-owned blocks
```

Дополнительная память — только двойной pinned staging buffer. Если selectors
переиспользуются, их следует держать резидентно в VRAM; если используется один
проход, передачу надо перекрывать вычислением через два или более stream.

## Запись результата

Не следует создавать промежуточный `Vec` координат. Callback сканера должен
сразу:

- очищать бит целевого решета;
- выполнять атомарное `fetch_and` для параллельного битсета;
- либо записывать в локальный сегмент потока.

`count_ones` полезен для определения плотности сегмента, но не заменяет
установку или очистку целевых битов.

## SIMD

32-байтовый `E8MaskBlock` помещается в один AVX2-регистр, но SIMD следует
добавлять только после фиксации layout целевого решета.

В горячем цикле не вычисляются:

- матрицы \(E_8\);
- кватернионы;
- координаты вершин;
- преобразования политопов.

Они используются для построения и проверки адресации. Runtime исполняет только
целочисленную арифметику и битовые операции.

## Реализация

Rust-crate:

`rust/e8_mask_codec/`

Проверка:

```bash
cd rust/e8_mask_codec
cargo test --release
cargo run --release --bin bench_scan
cargo run --release --bin bench_gpu
cargo run --release --bin bench_hybrid
cargo run --release --bin memory_report
```

## Запуск, остановка и просмотр прогресса

Теперь основной long-run сценарий запускается через корневой `Makefile`.

### Сборка

```bash
make prime-build
```

### Старт или продолжение вычисления

```bash
make count-start
```

По умолчанию это эквивалентно запуску:

- цели `100000000000` первых простых;
- `backend=cpu`;
- `threads=8`;
- `segment_blocks=4096`;
- checkpoint/progress/archive в `data/runs/count_1e11/`.

При наличии checkpoint рантайм продолжает run, а не считает с нуля.

### Запуск с другими параметрами

```bash
make count-start TARGET_N=100000000000 BACKEND=cpu THREADS=8 SEGMENT_BLOCKS=4096
```

### Корректная остановка

```bash
make count-stop
```

Отправляется `SIGTERM`, рантайм дописывает checkpoint и затем останавливается.

### Просмотр прогресса

```bash
make count-status
```

Статус теперь показывает:

- `progress_percent`;
- `remaining_percent`;
- `eta_min`;
- steady-state скорости;
- `archive_exact_gib`;
- `archive_wheel210_gib`;
- `archive_total_gib`.

### Просмотр лога

```bash
make count-log
```

### Перестроение `wheel210`-архива без пересчёта простых

```bash
make count-wheel210-rebuild
```

Этот режим использует уже накопленный exact-архив и достраивает sidecar `wheel210/`.


### Wheel210-only режим хранения

Текущий production-режим архива должен использовать `--archive-mode wheel210`, чтобы не
держать на диске одновременно exact delta-coded архив и `wheel210` sidecar.

Именно `wheel210-only` соответствует целевой оценке порядка `20–40 GiB` на `10^11`
простых. Режим `both` сохраняется только как переходный/отладочный.
