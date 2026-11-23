# duck-office

Suíte office nativa e focada em performance, para Windows, Linux e macOS. O primeiro app é o **duck-sheets** (planilhas compatíveis com `.xlsx`/`.xlsm`), feito para abrir arquivos gigantes sem travar.

Roadmap e status das fases: [ROADMAP.md](ROADMAP.md) · Decisões de arquitetura: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) · Medições: [docs/BENCHMARKS.md](docs/BENCHMARKS.md)

## Estrutura

```
apps/
  duck-sheets/       o editor de planilhas (F2, F3)
platform/            biblioteca nativa independente (não depende da suíte)
  duck-platform/     janela, entrada, DPI, clipboard, diálogos: chamadas ao SO
  duck-gfx/          renderer 2D por instâncias sobre wgpu
  duck-text/         shaping, estilos, quebra de linha, atlas de glifos
  duck-ui/           painter e widgets (campo de texto, abas)
  examples/
    million_rows/    grade de 1.000.000 de linhas (prova da F0)
crates/              a suíte office
  duck-core/         modelo do workbook: blocos colunares, strings, estilos, formatos de número,
                     edição e comandos com desfazer
  duck-par/          pool de threads, limitador de memória, cortes SIMD
  duck-deflate/      descompactação deflate em paralelo (especulativa)
  duck-xlsx/         leitura e gravação de .xlsx/.xlsm (em paralelo, gravação por remendo) e de .csv
  duck-calc/         fórmulas: referências, modelos relativos e ajuste ao inserir/apagar linhas
  duck-emf/          imagens EMF (registros GDI) para pixels
  duck-vba/          macros: projeto VBA do arquivo, analisador, interpretador e objetos do Excel
  duck-dax/          Modelo de Dados: definições, medidas DAX e consultas das dinâmicas
  duck-bench/        medição e conferência: open, sweep, verify, roundtrip, calc, dinamicas, modelo,
                     objetos, regress, gen, gen-showcase, inflate, preview
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

Sem arquivo, abre uma planilha em branco (Ctrl+O abre um arquivo, ou arraste-o para a janela). Teclas: setas (+Shift seleciona, +Ctrl vai até a borda dos dados), PageUp/PageDown, Ctrl+Home/End, Ctrl+PageUp/PageDown (abas), Ctrl+C (copia a seleção), Ctrl+F e F3 (busca), Ctrl+G ou F5 (ir para), Ctrl+O (abrir), Ctrl+A, Ctrl + roda ou Ctrl +/−/0 (zoom; também no controle do canto inferior direito).

A faixa de opções em cima tem as abas **Arquivo** (novo, abrir, recentes, salvar, salvar como, "Abrir com" do sistema), **Página Inicial** (área de transferência, fonte, alinhamento, número, células, edição), **Inserir** (linhas, colunas, mesclagem) e **Exibir** (zoom, linhas de grade, cabeçalhos, congelar painéis). O clique direito na grade ou nos cabeçalhos abre um menu com as ações mais comuns.

Edição: digitar substitui a célula; F2, duplo clique ou clique na barra de fórmulas editam o conteúdo; Enter/Tab confirmam e Esc cancela; Delete apaga a seleção; Ctrl+C, Ctrl+X e Ctrl+V copiam, recortam e colam (de outros programas também); Ctrl+Z e Ctrl+Y desfazem e refazem; Ctrl+S salva e Ctrl+Shift+S salva como. A barra de ferramentas tem negrito, itálico, sublinhado, tachado, cores, alinhamento, formatos de número, bordas e inserir/apagar linhas e colunas (também Ctrl+N/B, Ctrl+I, Ctrl+U, Ctrl+5, Ctrl+Shift+1/3/4/5/7 e Ctrl+Alt+= / Ctrl+Alt+-).

O que se digita é lido como no Excel em português: `1.234,56`, `10%`, `R$ 10`, `22/11/2026`, `14:30`, `VERDADEIRO`, `=A1*2`. As fórmulas digitadas são calculadas na hora (e o que depende delas acompanha); uma função que o motor ainda não tem fica sem valor e o arquivo salvo pede ao Excel para recalcular ao abrir.

Objetos: formas, imagens e controles com link ou macro respondem ao clique; os botões +/− dos grupos abrem e fecham linhas e colunas. As macros VBA do arquivo rodam num interpretador próprio, que pergunta uma vez por arquivo e só enxerga a planilha (sem arquivos, internet ou outros programas); Ctrl+Z desfaz a macro inteira.

Opções: `--sheet N` (aba inicial, 1 = primeira), `--cell B12`, `--zoom 80`, `--find texto`, `--novo` (planilha em branco). Para testar a edição sem a janela: `--edit A1=valor` (também `copiar:A1:B2`, `colar:D5`, `inserir:A5:A6`, `formatar:negrito:A1:B2`, `mesclar-centralizar:A1:C1`, `ocultar-linhas:A3:A4`, `congelar:B2`, `largura=20:C1`...), `--undo N`, `--save-to cópia.xlsx`; e, para screenshots, `--aba exibir` e `--menu bordas` (abre um menu).

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

Salvar numa cópia e conferir (partes intactas idênticas byte a byte, abas regravadas relidas e comparadas célula a célula, cópia conferida contra o calamine). `--rewrite-all` escreve toda célula a partir do modelo; `--edit N` faz N edições; `--estrutura` insere e apaga linhas e colunas:

```
cargo run --release -p duck-bench -- roundtrip caminho/arquivo.xlsx --edit 500
```

Conferir de uma vez o que já funciona (abertura, memória, fórmulas, dinâmicas, Modelo de Dados, gráficos, links, macros, salvar) contra a linha de base, e falhar se algo piorou:

```
cargo run --release -p duck-bench -- regress
cargo run --release -p duck-bench -- regress --gravar
cargo run --release -p duck-bench -- regress vitrine.xlsx --base scripts/bench/vitrine.txt
```

Sem arquivo, usa as linhas `regress = "..."` do `bench.local.toml` e a base `bench.baseline.txt` (os dois fora do git). Com `--base`, só o que não depende da máquina: é o que o CI confere com a vitrine.

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

## Licença

[MIT](LICENSE). Grátis para usar, copiar, modificar e distribuir.
