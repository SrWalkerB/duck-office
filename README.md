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
  duck-bench/        medição e conferência: open, sweep, verify, roundtrip, conferir-pacote, calc,
                     dinamicas, modelo, objetos, regress, gen, gen-showcase, inflate, preview
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

A faixa de opções em cima tem as abas **Arquivo** (novo, abrir, recentes, salvar, salvar como, "Abrir com" do sistema), **Página Inicial** (área de transferência, fonte, alinhamento, número, células, edição), **Inserir** (linhas, colunas, mesclagem) e **Exibir** (zoom, linhas de grade, cabeçalhos, congelar painéis, tema claro, escuro ou o do sistema). O clique direito na grade ou nos cabeçalhos abre um menu com as ações mais comuns.

Edição: digitar substitui a célula; F2, duplo clique ou clique na barra de fórmulas editam o conteúdo; Enter/Tab confirmam e Esc cancela; Delete apaga a seleção; Ctrl+C, Ctrl+X e Ctrl+V copiam, recortam e colam (de outros programas também); Ctrl+Z e Ctrl+Y desfazem e refazem; Ctrl+S salva e Ctrl+Shift+S salva como. Alt+Enter quebra a linha dentro da célula, Ctrl+Enter preenche a seleção toda com o que foi digitado e um clique no texto do editor põe o cursor ali. A barra de ferramentas tem negrito, itálico, sublinhado, tachado, cores, alinhamento, formatos de número, bordas e inserir/apagar linhas e colunas (também Ctrl+N/B, Ctrl+I, Ctrl+U, Ctrl+5, Ctrl+Shift+1/3/4/5/7 e Ctrl+Alt+= / Ctrl+Alt+-).

Ao digitar uma fórmula, na célula ou na barra, aparece a lista de funções, nomes, tabelas e colunas de tabela que começam com o que foi escrito (sem diferenciar maiúsculas nem acentos): ↑ e ↓ escolhem, Tab ou o clique inserem, Esc fecha a lista e Enter confirma a célula. Dentro dos parênteses, a assinatura da função mostra o argumento atual em destaque.

Com a fórmula aberta, clicar numa célula escreve a referência dela onde cabe uma (depois de `=`, operador, `(` ou `;`); em outro lugar, o clique confirma a edição. Arrastar ou Shift+clique faz um intervalo, o cabeçalho dá a coluna ou a linha inteira, e as setas também apontam (F2 alterna com andar no texto). O próximo clique troca a referência; depois de um operador ou `;`, começa outra. F4 alterna `$A$1`, `A$1`, `$A1` e `A1`. Para apontar em outra aba, clique na guia dela: confirmar volta para a aba da célula. Cada referência tem uma cor, no texto e na grade, e o parêntese que falta no fim é fechado ao confirmar.

Abas: o `+` no fim das guias (ou Shift+F11) cria, o duplo clique renomeia, arrastar move (com Ctrl, duplica) e o clique direito tem excluir, duplicar, mover, cor, ocultar e reexibir. As fórmulas, nomes definidos, gráficos, dinâmicas e segmentações acompanham, e tudo se desfaz com Ctrl+Z.

Preencher: arraste a alça no canto da seleção (13 vira 14, 15…; com Ctrl, só copia) ou dê duplo clique nela para ir até o fim da coluna vizinha; Ctrl+D e Ctrl+R preenchem para baixo e para a direita. As séries são as do Excel em português: números, datas, `jan`, `segunda-feira`, `Item 1`. Arrastar a borda da seleção move as células (com Ctrl, copia).

Colar especial (Ctrl+Alt+V): valores, fórmulas, formatos, larguras, sem bordas, transpor e operações; Ctrl+Shift+V cola só os valores. O pincel da Página Inicial copia o formato (duplo clique: várias vezes). Ctrl+H substitui na aba ou na pasta inteira, em fórmulas ou valores; Ctrl+1 abre Formatar células (número, alinhamento, fonte, borda e preenchimento).

Ctrl+clique e Ctrl+arrastar somam áreas à seleção, para formatar, limpar, preencher e copiar (quando as áreas se alinham, como no Excel); a caixa de nome aceita `A1:B2;D4`. A barra de status mostra soma, média, contagem, mínimo e máximo. Alt+= faz a AutoSoma, Ctrl+; e Ctrl+Shift+; escrevem a data e a hora, e inserir ou excluir células pode deslocar só as vizinhas.

No tema escuro a planilha também escurece, só na tela: as cores gravadas no arquivo não mudam. A escolha fica em `preferencias.txt`, na pasta de configuração do usuário (`%APPDATA%\duck-office` ou `~/.config/duck-office`).

O que se digita é lido como no Excel em português: `1.234,56`, `10%`, `R$ 10`, `22/11/2026`, `14:30`, `VERDADEIRO`, `=A1*2`. As fórmulas digitadas são calculadas na hora (e o que depende delas acompanha); uma função que o motor ainda não tem fica sem valor e o arquivo salvo pede ao Excel para recalcular ao abrir.

Objetos: formas, imagens e controles com link ou macro respondem ao clique; os botões +/− dos grupos abrem e fecham linhas e colunas. As macros VBA do arquivo rodam num interpretador próprio, que pergunta uma vez por arquivo e só enxerga a planilha (sem arquivos, internet ou outros programas); Ctrl+Z desfaz a macro inteira.

Opções: `--sheet N` (aba inicial, 1 = primeira), `--cell B12`, `--zoom 80`, `--find texto`, `--novo` (planilha em branco), `--tema claro|escuro|sistema` (só nesta execução; em `--screenshot` e `--bench`, sem isto, vale o claro). Para testar a edição sem a janela: `--edit A1=valor` (também `copiar:A1:B2`, `colar:D5`, `inserir:A5:A6`, `formatar:negrito:A1:B2`, `mesclar-centralizar:A1:C1`, `ocultar-linhas:A3:A4`, `congelar:B2`, `largura=20:C1`..., e as da tabela abaixo), `--undo N`, `--save-to cópia.xlsx`; e, para screenshots, `--aba exibir`, `--menu bordas` (abre um menu) e `--digitar "=so"` (digita na célula ativa e deixa a edição aberta; `{baixo}`, `{shift-direita}`, `{tab}`, `{enter}`, `{esc}`, `{f2}`, `{f4}`... são teclas, `{clique:B2}`, `{shift-clique:B5}`, `{arrastar:B2:B10}`, `{coluna:B}`, `{linha:3}` e `{aba:Nome}` são cliques, e `{barra}` no começo edita na barra de fórmulas).

| `--edit` | O que faz |
|---|---|
| `A1=um\ndois` | escreve com quebra de linha |
| `aba-nova[=Nome]`, `aba=N`, `aba-renomear=N:Nome`, `aba-excluir=N`, `aba-duplicar=N[:M]`, `aba-mover=N:M`, `aba-cor=N:RRGGBB`, `aba-ocultar=N`, `aba-reexibir=N` | abas (`aba=N` troca a aba das edições seguintes) |
| `preencher=A1:A2>A1:A10` (também `preencher-copia=`, `preencher-serie=`), `preencher-baixo:A1:A5`, `preencher-direita:A1:E1`, `preencher-duplo:A1:A2` | alça de preenchimento, Ctrl+D, Ctrl+R e duplo clique |
| `substituir=achar>trocar` (também com `-pasta`, `-valores`, `-exato`, `-celula` antes do `=`) | Ctrl+H |
| `colar-valores:D5`, `colar-formulas:`, `colar-formatos:`, `colar-larguras:`, `colar-sem-bordas:`, `colar-transpor:`, `colar-somar:`, `pincel:A1:B2>D1:E5` | colar especial e pincel |
| `mover=A1:B2>D5`, `copiar-arrastando=A1:B2>D5`, `inserir-celulas-baixo:A1:B2`, `inserir-celulas-direita:`, `excluir-celulas-cima:`, `excluir-celulas-esquerda:` | mover e deslocar células |
| `selecionar:A1:B2;D1`, `estatisticas:A1:B2;D1`, `preencher-todas=texto:A1:B3;D1`, `autosoma:A5`, `data:A1`, `hora:A2` | várias áreas, estatísticas, Ctrl+Enter, AutoSoma, data e hora |
| `formatar-celulas=codigo=0,00\|horizontal=centro>A1:B2;D1` | Formatar células: `codigo`, `categoria`, `casas`, `milhar`, `horizontal`, `vertical`, `recuo`, `orientacao`, `empilhado`, `quebrar`, `reduzir`, `mesclar`, `fonte`, `estilo`, `tamanho`, `cor`, `sublinhado`, `tachado`, `fundo`, `linha`, `corborda`, `borda` |

`--menu` também abre `guia`, `guia-cor`, `guia-renomear`, `colar`, `preencher`, `caixa-colar`, `caixa-substituir`, `caixa-inserir`, `caixa-excluir` e `caixa-formatar-0` a `-4`.

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
