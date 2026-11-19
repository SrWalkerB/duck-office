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
crates/              a suíte office
  duck-core/         modelo do workbook: blocos colunares, strings, estilos
  duck-par/          pool de threads, limitador de memória, cortes SIMD
  duck-deflate/      descompactação deflate em paralelo (especulativa)
  duck-xlsx/         leitura de .xlsx/.xlsm em paralelo e em streaming
  duck-calc/         fórmulas: referências e modelos relativos
  duck-bench/        medição: open, sweep, verify, gen, inflate
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

Teclas: setas, PageUp/PageDown, Home, Ctrl+setas, Ctrl+Home/End, Ctrl+G (linha do meio), Ctrl+C (copia a célula), F2 (liga/desliga vsync).

Outro tamanho de grade (padrão: 1 milhão de linhas):

```
cargo run --release -p million_rows -- --rows 10000000
```

Benchmark (abre a janela, mede e sai):

```
cargo run --release -p million_rows -- --bench --rows 10000000
```

Screenshot sem mostrar nada na tela:

```
cargo run --release -p million_rows -- --screenshot grade.png --row 500000
```

## Medindo a leitura de planilhas

Com um arquivo qualquer:

```
cargo run --release -p duck-bench -- open caminho/arquivo.xlsx
```

Curva de ganho por threads (um processo por rodada):

```
cargo run --release -p duck-bench -- sweep caminho/arquivo.xlsx
```

Conferir cada célula, fórmula e mesclagem contra o calamine (leitor independente):

```
cargo run --release -p duck-bench -- verify caminho/arquivo.xlsx
```

Comparar a descompactação paralela com o zlib-rs na maior parte do zip:

```
cargo run --release -p duck-bench -- inflate caminho/arquivo.xlsx
```

Gerar uma planilha sintética grande (600 mil linhas × 37 colunas por padrão):

```
cargo run --release -p duck-bench -- gen sintetica.xlsx --rows 600000
```

Sem caminho, o `duck-bench` usa `reference = "..."` de um `bench.local.toml` na raiz. Esse arquivo fica fora do git, porque a planilha de referência tem dados reais.

## Verificações

```
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
bash scripts/check-platform-deps.sh
```
