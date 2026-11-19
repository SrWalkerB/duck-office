# duck-office

Suíte office nativa e focada em performance, para Windows e Linux. O primeiro app é o **duck-sheets** (planilhas compatíveis com `.xlsx`/`.xlsm`), feito para abrir arquivos gigantes sem travar.

Decisões de arquitetura: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

## Estrutura

```
platform/            biblioteca nativa independente (não depende da suíte)
  duck-platform/     janela, entrada, DPI, clipboard: chamadas ao SO
  duck-gfx/          renderer 2D por instâncias sobre wgpu
  duck-text/         shaping, atlas de glifos, cache de texto
  examples/
    million_rows/    grade de 1.000.000 de linhas (prova da F0)
scripts/
  check-platform-deps.sh   garante que platform/ não depende da suíte
```

## Rodando

Requer Rust 1.95 ou mais novo. No Linux (Ubuntu/Debian), também:

```
sudo apt-get install build-essential libxkbcommon-x11-0
```

```
cargo run --release -p million_rows
```

Teclas: setas, PageUp/PageDown, Home, Ctrl+setas, Ctrl+Home/End, Ctrl+G (linha 500.000), Ctrl+C (copia a célula), F2 (liga/desliga vsync).

Benchmark (abre a janela, mede e sai):

```
cargo run --release -p million_rows -- --bench
```

Screenshot sem mostrar nada na tela:

```
cargo run --release -p million_rows -- --screenshot grade.png --row 500000
```

## Verificações

```
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
bash scripts/check-platform-deps.sh
```
