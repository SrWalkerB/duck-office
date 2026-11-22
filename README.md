# duck-office

Suíte office nativa e focada em performance, para Windows, Linux e macOS. O primeiro app é o **duck-sheets** (planilhas compatíveis com `.xlsx`/`.xlsm`), feito para abrir arquivos gigantes sem travar.

Roadmap e status das fases: [ROADMAP.md](ROADMAP.md) · Decisões de arquitetura: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

## Estrutura

```
apps/
  duck-sheets/       o visualizador de planilhas (F2)
platform/            biblioteca nativa independente (não depende da suíte)
  duck-platform/     janela, entrada, DPI, clipboard, diálogos: chamadas ao SO
  duck-gfx/          renderer 2D por instâncias sobre wgpu
  duck-text/         shaping, estilos, quebra de linha, atlas de glifos
  duck-ui/           painter e widgets (campo de texto, abas)
  examples/
    million_rows/    grade de 1.000.000 de linhas (prova da F0)
crates/              a suíte office
  duck-core/         modelo do workbook: blocos colunares, strings, estilos, formatos de número
  duck-par/          pool de threads, limitador de memória, cortes SIMD
  duck-deflate/      descompactação deflate em paralelo (especulativa)
  duck-xlsx/         leitura de .xlsx/.xlsm em paralelo e em streaming, e de .csv
  duck-calc/         fórmulas: referências e modelos relativos
  duck-bench/        medição: open, sweep, verify, gen, gen-showcase, inflate, preview
scripts/
  check-platform-deps.sh   garante que platform/ não depende da suíte
```

## Rodando

Requer Rust 1.95 ou mais novo. No Linux (Ubuntu/Debian), também:

```
sudo apt-get install build-essential libxkbcommon-x11-0
```

No macOS, as ferramentas de linha de comando do Xcode:

```
xcode-select --install
```

### duck-sheets

```
cargo run --release -p duck-sheets -- caminho/arquivo.xlsx
```

Sem arquivo, abre o diálogo nativo (ou arraste o arquivo para a janela). Teclas: setas (+Shift seleciona, +Ctrl vai até a borda dos dados), PageUp/PageDown, Ctrl+Home/End, Ctrl+PageUp/PageDown (abas), Ctrl+C (copia a seleção), Ctrl+F e F3 (busca), Ctrl+G ou F5 (ir para), Ctrl+O (abrir), Ctrl+A, Ctrl + roda ou Ctrl +/−/0 (zoom).

Opções: `--sheet N` (aba inicial, 1 = primeira), `--cell B12`, `--zoom 80`, `--find texto`.

Medir (abre, rola a aba, mostra os tempos e sai) e screenshot (renderiza fora da tela depois de carregar tudo):

```
cargo run --release -p duck-sheets -- caminho/arquivo.xlsx --bench
cargo run --release -p duck-sheets -- caminho/arquivo.xlsx --screenshot tela.png
```

`DUCK_TRACE=1` imprime os eventos e frames dos primeiros segundos.

Uma planilha de vitrine, sem dados reais, para conferir formatos, bordas e painéis:

```
cargo run --release -p duck-bench -- gen-showcase vitrine.xlsx
cargo run --release -p duck-sheets -- vitrine.xlsx
```

### million_rows

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

O caminho da primeira tela do duck-sheets (pacote, estilos, strings e prévia da aba ativa):

```
cargo run --release -p duck-bench -- preview caminho/arquivo.xlsx
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
