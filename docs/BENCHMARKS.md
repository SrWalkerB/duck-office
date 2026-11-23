# Medições do duck-sheets

Medido em **23/11/2026**. Estes são os números da [página do projeto](../site/index.html).

- **Máquina**: AMD Ryzen 9 5950X (16 núcleos), 64 GB, AMD Radeon RX 9070 XT (driver 32.0.32029.2010, DX12), Windows 11, janela de 1440×900.
- **Código**: `b194c5a` (F6), compilado com `--release`. A única diferença é que a janela de medição abre sem tomar o foco do teclado.
- **Como**: o computador estava em uso normal (editor, Docker, ~8% de CPU). Cada rodada só começa com a CPU abaixo de 15% e é descartada se houver compilação em paralelo. Nos tempos do app, uma rodada de aquecimento é descartada e vale a mediana.
- **Planilhas**: duas reais, do dia a dia de uma empresa (ficam fora do repositório; aqui só há contagens), e as sintéticas do `duck-bench gen`.

## Planilhas reais

| | Base de dados | Painel |
|---|---|---|
| Arquivo (`.xlsm`) | 194 MB, 1,9 GB de XML | 19 MB, 67 MB de XML |
| Abas | 20 | 21 |
| Células com valor | 24.171.714 | 1.338.957 |
| Fórmulas | 10.111.650 | 140.250 |
| Gráficos / segmentações / tabelas dinâmicas | 37 / 46 / 72 | 20 / 26 / 21 |

### Abrir (`duck-sheets --bench`, mediana de 5 rodadas [mínimo–máximo])

| | Base de dados | Painel |
|---|---|---|
| Primeira tela com dados | **228 ms** [216–251] | **272 ms** [268–275] |
| Todas as abas | **653 ms** [639–703] | **354 ms** [348–359] |
| Memória com tudo aberto | 499 MB | 289 MB |
| Pico de memória | 552 MB [541–557] | 426 MB [422–429] |
| Montagem do frame ao rolar, p99 | 1,4–1,8 ms | 0,2–0,3 ms |

Só a leitura (`duck-bench open`, 16 threads, 5 rodadas): **394 ms** [381–493] na base de dados e **288 ms** [280–291] no painel.

### Calcular (`duck-bench calc --recalc`, 3 rodadas, 32 threads)

| | Base de dados | Painel |
|---|---|---|
| Fórmulas iguais ao Excel | **10.111.650 de 10.111.650** | **140.249 de 140.249** (mais 1 com função não suportada, que mantém o valor gravado) |
| Recálculo completo | **671 ms** [634–685], 28 níveis | **8 ms**, 9 níveis |
| Só as voláteis | 240 ms (5,1 milhões de células) | menos de 1 ms |
| Valores que mudaram no recálculo | 0 | 0 |

### Salvar (`duck-bench roundtrip`)

O tempo inclui a conferência antes de trocar o arquivo. A cópia é comparada com o calamine, um leitor independente.

| | Base de dados | Painel |
|---|---|---|
| Sem mudanças | **0,20 s** (701 partes idênticas byte a byte) | 1,03 s (319 partes idênticas; 0,02 s de gravação + 1,0 s de conferência) |
| 500 edições | **2,6–2,9 s** (1,6–1,7 s de gravação + 1,0–1,1 s de conferência) | 1,1 s |
| Inserir e apagar linhas e colunas | 4,6 s para mudar, 3,5 s para salvar (1 rodada) | 0,02 s para mudar, 1,2 s para salvar |
| Divergências contra o calamine | **0** em todos os casos | **0** em todos os casos |

### Tabelas dinâmicas (`duck-bench dinamicas` e `modelo`)

| | Base de dados | Painel |
|---|---|---|
| Com registros no arquivo, layout idêntico ao Excel | **38 de 38** | **21 de 21** |
| Valores iguais / diferentes | 94.065 / 0 | 1.353 / 1 (valor antigo gravado no próprio arquivo) |
| Do Modelo de Dados | 29 de 34 com o layout idêntico, 28 iguais em tudo; 13.129 valores iguais, 15 diferentes; ~0,2 s para todas | — |

### Comparação com as versões anteriores

Mesma máquina, mesma hora, alternando as versões (6 rodadas de cada, base de dados). "F6" aqui é o código às 9h16, pouco antes do commit `b194c5a`:

| | `bac8782` (22/11, antes da edição de fórmulas e painéis) | `a3400b2` (versão 1.0) | F6 |
|---|---|---|---|
| Primeira tela com dados | 223 ms [211–232] | 227 ms [218–246] | 235 ms [221–252] |
| Todas as abas | 665 ms | 675 ms | 685 ms |
| Memória com tudo aberto | 434 MB | 494 MB | 494 MB |
| Pico de memória | 530 MB | 546 MB | 547 MB |

- **Tempo**: as diferenças ficam dentro da variação entre rodadas. Os 190–199 ms publicados em 21/11 não se repetem hoje nem com o código daquela época: abrir a GPU está levando mais tempo nesta máquina (186–221 ms até ela ficar pronta).
- **Memória**: o uso com tudo aberto subiu 60 MB (14%) e o pico 17 MB (3%), com as dinâmicas, os gráficos e o motor de cálculo.

## Planilhas sintéticas

`duck-bench gen`, 37 colunas e 10 abas pequenas, mediana de 5 rodadas depois do aquecimento:

| | 600 mil linhas (147 MB) | 100 mil linhas (26 MB) |
|---|---|---|
| Primeira tela com dados | 230 ms [224–244] | 234 ms [223–259] |
| Todas as abas | 2,33 s [2,28–2,33] | 0,63 s [0,62–0,66] |
| Memória com tudo aberto | 417 MB | 233 MB |
| Pico de memória | 607 MB [599–614] | 523 MB [519–551] |

## Busca

"sao", sem acento, numa aba de 60 mil linhas × 15 colunas (900 mil células, planilha de demonstração): 10.342 células em **4 ms**, igual em 5 rodadas.

## Comparação com o OnlyOffice

**Ainda não refeita para esta versão.** O comparativo da página é de 22/11/2026, num MacBook Air M1 com 8 GB, com o OnlyOffice 9.0.4:

| | duck-sheets: primeira tela / tudo | OnlyOffice: dados na tela |
|---|---|---|
| 600 mil linhas (147 MB) | 0,19 s / 2,3 s | 39,3 s |
| 100 mil linhas (26 MB) | 0,19 s / 0,55 s | 8,7 s |

## Refazer

```
cargo build --release -p duck-sheets -p duck-bench
target/release/duck-sheets ARQUIVO.xlsx --bench
target/release/duck-bench open ARQUIVO.xlsx
target/release/duck-bench calc ARQUIVO.xlsx --recalc
target/release/duck-bench roundtrip ARQUIVO.xlsx --edit 500
target/release/duck-bench dinamicas ARQUIVO.xlsx
python scripts/bench/vs_onlyoffice.py            # planilhas sintéticas, duck-sheets × OnlyOffice
python scripts/bench/vs_onlyoffice.py --local    # as planilhas `regress` de bench.local.toml
```

O `--bench` e o `vs_onlyoffice.py` abrem janelas que tomam o foco: não digite durante a medição.
