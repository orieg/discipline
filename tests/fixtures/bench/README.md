# Real Benchmark Tool Fixtures & Provenance

This directory stores real output artifacts produced by actual benchmark tools for verification in `discipline`'s `bench-regression` gate.

## 1. Criterion (`tests/fixtures/bench/criterion/`)
- **Tool**: `criterion 0.5` (Rust)
- **Artifacts**: `base/estimates.json`, `head/estimates.json`
- **Producing Command**:
  ```bash
  cargo bench --bench fib_bench
  ```
- **Provenance / Format Notes**:
  Criterion's `estimates.json` structures confidence intervals inside `mean.confidence_interval` using `lower_bound` and `upper_bound` with `confidence_level: 0.95`.

## 2. Go Benchmark (`tests/fixtures/bench/go/`)
- **Tool**: `go test` (Go 1.25.6 darwin/arm64)
- **Artifacts**: `base.txt`, `head.txt`
- **Producing Command**:
  ```bash
  go test -bench BenchmarkSearch -benchmem
  ```
- **Provenance / Format Notes**:
  Standard Go benchmark format reporting iterations, `ns/op`, and memory allocations.

## 3. pytest-benchmark (`tests/fixtures/bench/pytest/`)
- **Tool**: `pytest-benchmark 5.2.3` (Python 3.10)
- **Artifacts**: `base.json`, `head.json`
- **Producing Command**:
  ```bash
  pytest test_serialize.py --benchmark-json=pytest_bench.json --benchmark-min-rounds=50
  ```
- **Provenance / Format Notes**:
  `pytest-benchmark` JSON schema with `benchmarks[].stats.mean`.

## 4. Google Benchmark (`tests/fixtures/bench/google/`)
- **Tool**: Google Benchmark 1.9.5 (Clang++ C++17)
- **Artifacts**: `base.json`, `head.json`
- **Producing Command**:
  ```bash
  clang++ -O3 -std=c++17 bench.cpp -I/opt/homebrew/include -L/opt/homebrew/lib -lbenchmark -o bench
  ./bench --benchmark_format=json
  ```
- **Provenance / Format Notes**:
  Google Benchmark JSON format with `benchmarks[].cpu_time` and `time_unit: "ns"`.
