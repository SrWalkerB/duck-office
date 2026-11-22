# duck-office: decisões de arquitetura

> Status: **rascunho inicial** · Escopo: suíte duck-office, começando pelo **duck-sheets**
> Formato: cada decisão segue o modelo ADR curto (contexto → decisão → alternativas → consequências).

## Sumário

- [0. Visão](#0-visão)
- [1. Arquivo de referência e metas](#1-arquivo-de-referência-e-metas)
- [ADR-001 · Onde está o gargalo de verdade](#adr-001--onde-está-o-gargalo-de-verdade)
- [ADR-002 · Linguagem: Rust](#adr-002--linguagem-rust)
- [ADR-003 · Organização em crates](#adr-003--organização-em-crates)
- [ADR-004 · Modelo de dados colunar em blocos](#adr-004--modelo-de-dados-colunar-em-blocos)
- [ADR-005 · Carga de xlsx/xlsm em streaming](#adr-005--carga-de-xlsxxlsm-em-streaming)
- [ADR-006 · Fidelidade: nunca corromper um arquivo](#adr-006--fidelidade-nunca-corromper-um-arquivo)
- [ADR-007 · Camada nativa e UI como biblioteca independente](#adr-007--camada-nativa-e-ui-como-biblioteca-independente)
- [ADR-008 · Modelo de concorrência](#adr-008--modelo-de-concorrência)
- [ADR-009 · Camada de comandos](#adr-009--camada-de-comandos)
- [ADR-010 · VBA: preservar agora, executar depois](#adr-010--vba-preservar-agora-executar-depois)
- [ADR-011 · Plataformas](#adr-011--plataformas)
- [ADR-012 · Escalar com o hardware: paralelismo e SIMD](#adr-012--escalar-com-o-hardware-paralelismo-e-simd)
- [ADR-013 · Formato de arquivo nativo (futuro, em discussão)](#adr-013--formato-de-arquivo-nativo-futuro-em-discussão)
- [Roadmap](#roadmap)
- [Questões em aberto](#questões-em-aberto)

---

## 0. Visão

Uma suíte office **nativa**, **simples como o Google Sheets** e **rápida o suficiente para abrir arquivos que travam o Excel, o LibreOffice e o OnlyOffice**. Sem tecnologia web (nada de Electron, Tauri ou WebView). O primeiro produto é o **duck-sheets** (planilhas, compatível com `.xlsx`/`.xlsm`). Depois, outros apps (`duck-docs`, `duck-slides`) reutilizam a mesma base.

Princípios, em ordem de prioridade:

1. **Nunca travar a interface.** A UI responde a cada frame, mesmo com o arquivo ainda carregando.
2. **Nunca corromper um arquivo.** O que não entendemos, preservamos byte a byte.
3. **Pagar só pelo que se vê.** Trabalho proporcional ao que está na tela, não ao tamanho da planilha.
4. **Escalar com o hardware.** Quem tem mais núcleos e CPU mais nova abre e calcula mais rápido; quem tem menos continua fluido (ADR-012).
5. **Medir antes de otimizar.** Toda meta de performance tem benchmark automatizado.

---

## 1. Arquivo de referência e metas

O caso que motivou o projeto é um `.xlsm` real que o OnlyOffice não abre. Inspeção estrutural (só leitura):

| Métrica | Valor |
|---|---|
| Tamanho no disco (compactado) | **194 MB** |
| Maior aba, XML descompactado | **1,64 GB** |
| Dimensão da maior aba | `A1:AK593558` (~593 mil linhas × 37 colunas ≈ **22 milhões de células**) |
| `calcChain.xml` | 228 MB (milhões de fórmulas) |
| Maior pivot cache | 281 MB de XML |
| Abas | 20 |
| Tabelas dinâmicas | 144 (50 pivot caches) |
| Gráficos | 148 |
| Slicers / timelines | 35 slicer caches + timelines |
| Outros | Power Pivot (`xl/model/item.data`), VBA (`vbaProject.bin`), controles de formulário, `connections.xml`, 53 partes `customXml`, imagens EMF |

Esse arquivo é o **benchmark principal**. Ele contém dados de empresa e **não entra no repositório**: o caminho fica numa configuração local (`bench.local.toml`, no `.gitignore`), e o CI usa um **gerador sintético** com o mesmo perfil (mesmo número de células, proporção de fórmulas compartilhadas, strings repetidas, estilos).

### Metas iniciais (hipóteses a validar na F1)

| Métrica | Meta |
|---|---|
| Primeira tela visível após abrir | **< 1,5 s** |
| Carga completa da maior aba | **< 8 s** em CPU de 8 núcleos; **< 4 s** com 16+ núcleos |
| Escalabilidade da carga | speedup **≥ 5×** com 8 threads vs 1 thread |
| Memória residente com o arquivo aberto | **< 1,5 GB** |
| Scroll e navegação | **60+ fps** (frame < 16 ms), UI thread nunca bloqueia > 1 frame |
| "Ir para a linha 500000" | **< 1 frame** |
| Startup a frio do app sem arquivo | **< 300 ms** |

### Metas revisadas depois da F1

A F1 leu o arquivo inteiro em ~0,65 s, bem abaixo das hipóteses iniciais. As metas passam a ser:

| Métrica | Medido na F1 (Ryzen 9 5950X) | Nova meta |
|---|---|---|
| Carga completa do arquivo de referência | 0,62–0,65 s (8+ threads); 3,95 s (1 thread) | **< 1 s** com 8+ núcleos; **< 5 s** com 1 núcleo; **< 0,4 s** com 16+ núcleos depois do inflate paralelo |
| Escalabilidade | 6,1× com 8 threads; satura em ~6,4× | **≥ 10×** com 16 threads (exige inflate paralelo) |
| Memória com o arquivo aberto | ~460–490 MB | **< 600 MB** |
| Pico de memória durante a leitura | ~0,9–1,1 GB | **< 1 GB** |
| Primeira tela visível após abrir (F2) | — | **< 300 ms** (aba ativa primeiro, carga progressiva) |

Situação depois de resolver as pendências da F1 (agora lendo também fórmulas e layout, o que aumentou o trabalho total em ~25%):

| Meta | Windows | Linux | |
|---|---|---|---|
| < 1 s com 8+ núcleos | 785 ms (8 thr) | 852 ms (8 thr) | ✓ |
| < 5 s com 1 núcleo | 4,85 s | 4,8–5,0 s | ✓ no limite |
| < 0,4 s com 16+ núcleos | 528 ms (32 thr) | 543 ms (32 thr) | ✗ (era 650 ms) |
| ≥ 10× com 16 threads | 8,6× (16), 9,2× (32) | 8,6× (16), 9,2× (32) | ✗ (era 6,4×) |
| < 600 MB com o arquivo aberto | ~510 MB | ~490 MB | ✓ |
| Pico < 1 GB | 615–936 MB | 563–746 MB | ✓ |

As duas metas não cumpridas têm a mesma causa: a leitura deixou de ter um trecho serial e agora é **limitada pela CPU total** (~10 s de CPU somados, ~160 ns por célula no parse). O próximo ganho vem de deixar o laço principal do parse mais barato, não de mais paralelismo.

Depois da otimização do parse (19/11/2026; detalhes em "Otimização do parse", no fim do status da F1). A máquina tinha ~7–10% de CPU ocupada por outros programas durante as medições, e os números variam ~10% entre rodadas:

| Meta | Windows | Linux | |
|---|---|---|---|
| < 5 s com 1 núcleo | **3,39–3,44 s** | **3,37 s** | ✓ (era 4,85 s) |
| < 1 s com 8+ núcleos | 570–668 ms (8 thr) | 676 ms (8 thr) | ✓ |
| < 0,4 s com 16+ núcleos | 441–560 ms (16–32 thr, conforme a carga da máquina) | 449–497 ms | ✗ (a aba gigante sozinha: 409 ms) |
| ≥ 10× com 16 threads | ~7× | ~6,8× | ✗ (a base de 1 thread ficou 30% mais rápida) |
| < 600 MB com o arquivo aberto | 477 MB (padrão: 16 thr) | 457 MB | ✓ |
| Pico < 1 GB | 817–838 MB (padrão); 935–1.020 MB com 32 thr | 641–676 MB (padrão); 728–762 MB com 32 thr | ✓ no padrão |

O pool padrão agora usa **uma thread por núcleo físico** (16 nesta máquina): 32 threads não ganharam tempo sobre 16 e aumentaram o pico de memória.

---

## ADR-001 · Onde está o gargalo de verdade

**Contexto.** A intuição inicial é que "Electron/Tauri deixam lento". Isso é verdade para RAM e startup, mas não explica por que os editores atuais travam nesse arquivo: o LibreOffice e o OnlyOffice não são web puro e mesmo assim travam.

**Decisão.** Tratar o **motor** como o problema principal e a **casca nativa** como requisito secundário (importante, mas não é onde mora o ganho de 10×). Os editores atuais travam por:

- Parse com **DOM** (o XML inteiro vira árvore de objetos na memória: 1,64 GB de XML viram vários GB de RAM).
- **Um objeto por célula** (22M alocações, ponteiros, cache misses).
- **Recalcular ao abrir** (milhões de fórmulas antes de mostrar qualquer coisa).
- **Carregar tudo antes de pintar** (o usuário vê uma tela congelada).
- Layout/renderização proporcional à planilha, não à janela.

**Consequências.** As primeiras fases focam no modelo de dados (ADR-004) e na carga em streaming (ADR-005). A UI nativa (ADR-007) existe para tirar o custo fixo de um navegador embutido (RAM, latência de input, startup) e dar controle total da renderização da grade.

---

## ADR-002 · Linguagem: Rust

**Contexto.** Precisamos de performance no nível de C/C++, sem GC (sem pausas durante o scroll), paralelismo de verdade e segurança: o app vai fazer parse de arquivos de terceiros, que é a superfície clássica de vulnerabilidades em suítes office.

**Decisão.** **Rust** em todo o projeto.

- Performance de C/C++, SIMD, controle de layout de memória.
- Segurança de memória por padrão nos parsers.
- Paralelismo seguro (`rayon`, canais, `Send`/`Sync` verificados pelo compilador).
- Ecossistema que cobre exatamente o que precisamos: `quick-xml`, `zip`, `zlib-rs`/`libdeflater`, `memmap2`, `winit`, `wgpu`, `cosmic-text`, `swash`, `accesskit`.
- Um único toolchain (`cargo`) para Windows e Linux.

**Alternativas rejeitadas.**

| Opção | Por que não |
|---|---|
| C++ | Mesma performance, mas segurança de memória por disciplina e build multiplataforma bem mais custoso. |
| Zig | Promissor, mas linguagem pré-1.0 e ecossistema imaturo para texto, GPU e janelas. |
| C# / Java / Go | GC e pausas; controle de memória insuficiente para 22M células compactas. |

**Consequências.** Curva de aprendizado maior; tempo de compilação precisa ser cuidado (crates pequenas, `opt-level` alto só nas dependências quentes em debug).

---

## ADR-003 · Organização em crates

**Decisão.** Um workspace Cargo com crates de responsabilidade única:

```
duck-office/
├── platform/              # camada nativa + UI: biblioteca independente, sem nada de office (ADR-007)
│   ├── duck-platform      # janela, input, IME, clipboard, diálogos, fontes, DPI: chamadas ao SO
│   ├── duck-gfx           # renderer 2D por instâncias (GPU) + fallback por software
│   ├── duck-text          # shaping, atlas de glifos, cache de texto
│   ├── duck-ui            # widgets retidos, layout, temas
│   └── examples/          # hello_window, million_rows, ... (provam que roda sozinha)
├── crates/                # a suíte office
│   ├── duck-par           # infraestrutura de paralelismo e kernels SIMD (ADR-012)
│   ├── duck-deflate       # descompactação deflate em paralelo (ADR-005)
│   ├── duck-core          # modelo do workbook: células, blocos, strings, estilos
│   ├── duck-xlsx          # leitura (v1) e escrita (v2) de xlsx/xlsm em streaming
│   ├── duck-calc          # fórmulas: referências e modelos relativos (hoje); parser, AST e cálculo (F4)
│   ├── duck-sheets        # o app: grade virtualizada, barra de fórmulas, abas
│   └── duck-bench         # CLI de benchmark + gerador de planilhas sintéticas
└── docs/
```

Futuras: `duck-vba`, `duck-docs`, `duck-slides`, todas reutilizando `duck-core` (quando fizer sentido) e a camada `platform/`.

**Regras de dependência.**

- **`platform/*` nunca depende de `crates/*`.** É uma biblioteca de uso geral; pode ser extraída para um repositório próprio sem mudar uma linha. O CI verifica essa regra.
- Dentro de `platform/`, as camadas só dependem para baixo: `duck-ui` → `duck-text` / `duck-gfx` → `duck-platform`. Quem quiser só uma janela com clipboard e diálogos usa `duck-platform` sozinho.
- `duck-core` não depende de nada de UI nem de formato de arquivo. `duck-xlsx` e `duck-calc` dependem só de `duck-core` e `duck-par`. Só `duck-sheets` junta tudo. Isso permite rodar carga e cálculo sem janela (benchmarks, testes, CLI de conversão).

---

## ADR-004 · Modelo de dados colunar em blocos

**Contexto.** 22M células como objetos individuais custam facilmente 2–4 GB e destroem o cache da CPU. Planilhas reais são muito repetitivas: colunas inteiras do mesmo tipo, as mesmas strings milhares de vezes, a mesma fórmula copiada para baixo.

**Decisão.**

- **Armazenamento colunar em blocos**: cada coluna é dividida em blocos de 1024 linhas (tamanho a calibrar). Cada bloco guarda vetores tipados:
  - números: `f64`
  - texto: `u32` (id na tabela de strings)
  - estilo: `u32` (id na tabela de estilos), com compressão run-length (estilos se repetem em sequência)
  - tipo e vazio: bitmap/tags compactos
- **Blocos vazios não alocam.** Uma coluna com 10 valores ocupa 1 bloco, não 593 mil entradas.
- **Strings internadas**: a `sharedStrings` do xlsx vira uma tabela única; células guardam só o `u32`.
- **Estilos deduplicados**: combinações iguais de fonte/borda/preenchimento/formato viram um id só.
- **Fórmulas compartilhadas como grupo**: uma fórmula copiada 500 mil vezes é guardada como **um template (referências relativas, estilo R1C1) + o intervalo** onde se aplica, não 500 mil strings.
- **Valores em cache do arquivo são usados para exibir.** O xlsx já traz o último resultado de cada fórmula. Na abertura **nada é recalculado**: mostramos os valores salvos e o grafo de dependências é montado depois, em background, só quando necessário.

**Alternativas rejeitadas.**

- *Objeto por célula / `HashMap<(row, col), Cell>`*: simples, mas memória e cache misses inaceitáveis nessa escala.
- *Banco embutido (SQLite/DuckDB) como armazenamento*: ótimo para consultas, mas adiciona latência por acesso e não casa com o padrão de leitura da grade. Pode voltar como ferramenta para pivôs/consultas no futuro.

**Consequências.** Estimativa para o arquivo de referência: **~300–500 MB** de RAM para as células. Inserir uma linha no meio exige deslocar blocos (aceitável: é uma operação por bloco, não por célula). O acesso aleatório é O(1): `bloco = linha / 1024`.

---

## ADR-005 · Carga de xlsx/xlsm em streaming

**Contexto.** Um `.xlsx` é um zip de XMLs. A maior aba do arquivo de referência tem 1,64 GB de XML descompactado. Ler isso para a memória ou montar DOM é inviável.

**Decisão.**

1. **Arquivo mapeado em memória** (`mmap`) e zip lido direto do mapeamento, sem cópia.
2. **Pipeline por aba com parse paralelo em pedaços**:

   ```
                              ┌─▶ [parser de linhas, thread 1] ─▶ blocos 0..k   ─┐
   [inflate] ──fatias ~4 MB──┼─▶ [parser de linhas, thread 2] ─▶ blocos k..m   ─┼─▶ [emenda dos blocos]
   libdeflate/zlib-rs        └─▶ [parser de linhas, thread N] ─▶ blocos m..n   ─┘     duck-core
   ```

   - O inflate produz fatias do XML descompactado. Cada fatia é cortada numa fronteira `<row ` (achada com busca SIMD), então **cada pedaço é autocontido** e vai para um núcleo diferente.
   - As linhas trazem o próprio número (`r="12345"`), e as células só guardam ids de string e de estilo. Por isso cada thread monta os seus blocos sem esperar as outras, e a emenda final é só ligar ponteiros, não copiar dados.
   - O inflate de uma parte é sequencial por natureza. Para partes grandes (≥ 8 MB comprimidos) entra o **inflate paralelo especulativo** do `duck-deflate` (técnica do rapidgzip): decodificação de várias partes ao mesmo tempo sem conhecer a janela anterior, marcadores resolvidos em ordem, conferência pelo CRC32 do zip e volta automática ao zlib-rs se algo não bater. Cada tarefa lê as linhas da própria parte em pedaços de 1 MB à medida que descompacta, e os blocos que ficam inteiros já são compactados ali: nem o XML descompactado nem a aba sem compactar ficam inteiros na memória.

3. **Partes independentes em paralelo**: `sharedStrings`, `styles` e todas as abas carregam ao mesmo tempo (as células só referenciam ids, então não há dependência de ordem). **A aba ativa tem prioridade** no escalonador. A grade é pintada assim que as primeiras linhas chegam (**carga progressiva**); o resto continua em background e a barra de rolagem cresce conforme carrega.
4. **Abas não visitadas podem ficar lazy** (carregadas ao clicar), com pré-carga em background quando a CPU estiver livre.
5. **Ignorado na leitura**: `calcChain.xml` (é só uma dica de ordem de cálculo; reconstruímos quando houver recálculo).
6. **Lazy ou só preservado**: pivot caches, data model do Power Pivot, slicers, timelines.
7. Parsers escritos à mão sobre `quick-xml` para os elementos quentes (`<row>`, `<c>`, `<v>`, `<f>`), sem serde genérico nem alocação por célula.

**Otimizações futuras.**

- **`.xlsb`** (formato binário do Excel): ordens de grandeza mais rápido de ler; bom alvo para "salvar como formato rápido".
- **`.ods`** (o `.csv` já abre desde a F2).
- **Formato nativo** (ADR-013) para reabrir instantaneamente, sem XML.

**Consequências.** Com o parse distribuído entre os núcleos, o tempo de carga passa a ser dominado pelo inflate (~1 GB/s por núcleo com libdeflate), o que torna a meta de < 8 s plausível e a de < 4 s alcançável com inflate paralelo. A UI precisa lidar com "planilha ainda carregando" como estado normal.

---

## ADR-006 · Fidelidade: nunca corromper um arquivo

**Contexto.** O arquivo de referência tem dezenas de recursos que o duck-sheets não vai entender tão cedo (VBA, Power Pivot, slicers, web extensions, controles de formulário). Se salvarmos e perdermos isso, o usuário perde o trabalho no Excel.

**Decisão.**

- Toda parte do pacote que não interpretamos é guardada como **bytes brutos** junto com seus relacionamentos (`.rels`) e regravada **intacta** ao salvar.
- Partes que interpretamos parcialmente guardam os atributos/elementos desconhecidos para reemissão.
- Um teste de **round-trip** (abrir → salvar → comparar partes) roda no CI para cada arquivo do corpus.
- Na dúvida, o app **avisa e oferece "salvar como cópia"** em vez de sobrescrever.

**O que aparece na v1, mesmo sem suporte específico.** A saída das tabelas dinâmicas já está gravada como células comuns na aba, então ela **aparece normalmente** (sem interatividade). Gráficos, slicers e imagens entram em fases posteriores.

---

## ADR-007 · Camada nativa e UI como biblioteca independente

**Contexto.** Requisitos: nativo, sem tecnologia web, Windows + Linux (X11 e Wayland), macOS depois, e uma grade capaz de rolar 22M células a 60+ fps. Nenhum toolkit pronto tem uma grade de planilha nesse nível; ela seria custom de qualquer forma. Além disso, a camada que conversa com o sistema operacional é útil **fora da suíte office**: outros apps desktop, ferramentas internas, visualizadores, utilitários.

**Decisão.** Construir uma **biblioteca própria e independente** em `platform/`, separada em camadas que podem ser usadas isoladamente. Ela não sabe o que é uma planilha; o duck-sheets é apenas o primeiro cliente.

| Crate | Responsabilidade | Base |
|---|---|---|
| `duck-platform` | **Tudo que é chamada nativa ao SO**: janelas, input, eventos, IME, clipboard, diálogos de arquivo, fontes do sistema, DPI, cursor, drag-and-drop, (depois) notificações, bandeja, associação de arquivos | `winit` + `rfd` + `arboard` + APIs nativas diretas onde faltar |
| `duck-gfx` | Renderer 2D por **instâncias**: retângulos, bordas, linhas, imagens e glifos em 1–3 draw calls por frame | `wgpu` com **um backend nativo por SO** (DX12 no Windows, Vulkan no Linux, Metal no macOS; os outros só como fallback); sem GPU, o adaptador por software do sistema (WARP / llvmpipe); `tiny-skia` + `softbuffer` se isso não bastar |
| `duck-text` | Shaping, bidi, fallback de fontes, rasterização, atlas de glifos e **cache de texto moldado por id de string** | `cosmic-text` + `swash` |
| `duck-ui` | Árvore de widgets retida, layout, foco, temas, acessibilidade | próprio + `accesskit` |

**Por que envolver `winit` & cia. em vez de chamá-los direto.** O `duck-platform` expõe **uma API nossa, estável**. Por trás dela, cada backend pode ser trocado por chamadas diretas ao Win32, X11/Wayland ou Cocoa quando um profiling mostrar ganho, ou quando a lib usada não cobrir algo (ex.: diálogos nativos específicos, notificações), sem mudar nenhum app. Reescrever o gerenciamento de janelas do zero hoje custaria meses sem ganho de performance, já que não é caminho quente.

**Como manter a separação real.**

- `platform/*` não depende de nada da suíte (regra verificada no CI).
- Exemplos próprios (`hello_window`, `million_rows`, `text_stress`) compilam e rodam sem nenhum crate de office.
- Documentação e versionamento (semver) próprios; pode virar repositório separado quando estabilizar.

**A grade virtualizada** fica em `duck-sheets` (é específica de planilha), mas só usa a API pública de `platform/`. É o componente central do app:

- Processa **só as células visíveis**: custo O(células na tela), nunca O(planilha).
- Larguras de colunas e alturas de linhas em **árvore de Fenwick (prefix sums)**: "qual linha está no pixel Y" e "ir para a linha 500000" em O(log n).
- Frame = coletar células visíveis dos blocos → montar instâncias → 1 upload → poucos draw calls.
- Congelar painéis, merges e seleção são camadas sobre a mesma passada.

**O resto da casca** (menus, toolbar, abas, barra de fórmulas, diálogos) usa uma **árvore de widgets retida e mínima**, que redesenha só o que mudou e fica ociosa (0% de CPU) quando nada acontece.

**Alternativas avaliadas.**

| Opção | Por que não |
|---|---|
| Electron / Tauri | Navegador embutido: RAM, startup e latência de input. Requisito explícito do projeto. |
| Qt | Maduro e nativo, mas C++, pesado, licenciamento LGPL/comercial, e a grade seria custom do mesmo jeito. |
| egui | Fácil e rápido, mas modo imediato com pouco controle sobre texto e sobre o pipeline da grade. |
| GPUI (Zed) | Muito rápido e bem desenhado, mas API instável e acoplada ao editor Zed. **Serve de referência de design.** |
| Slint / iced | Bons toolkits, mas a peça difícil (grade) continuaria custom e herdaríamos limitações de layout e texto. |

**Consequências.** Mais trabalho inicial em widgets básicos (botão, menu, campo de texto com IME) e disciplina para manter a API de `platform/` genérica. Em troca: controle total do frame, sem dependência de motor web, binário pequeno, e uma biblioteca que serve para os próximos apps da suíte **e para projetos fora dela**.

---

## ADR-008 · Modelo de concorrência

**Decisão.**

- **UI thread** só trata input e desenha. Proibido I/O, parse ou cálculo nela.
- **Workers** (`rayon` + threads dedicadas) para carga, busca, cálculo (v2) e exportação. Como dividir o trabalho entre eles está no ADR-012.
- Comunicação por **mensagens** (canais). Resultados chegam como eventos que a UI processa no próximo frame.
- A UI lê **snapshots imutáveis** dos blocos (versionados por epoch, estilo copy-on-write). Ela nunca espera um lock longo; no pior caso, desenha a versão anterior por um frame.
- Operações longas são **canceláveis** e reportam progresso.

---

## ADR-009 · Camada de comandos

**Decisão.** Toda mudança no workbook passa por **comandos/transações** (`SetCellValue`, `InsertRows`, `ApplyStyle`, ...). Nenhum código altera os blocos diretamente.

**Consequências.**

- **Undo/redo** sai de graça (cada comando sabe se inverter).
- A edição da v2 se apoia nisso desde o início.
- Base para **colaboração** no futuro (comandos são serializáveis).
- É a **mesma API que o VBA vai chamar** (ADR-010): UI e scripts usam o mesmo caminho, com as mesmas regras.

---

## ADR-010 · VBA: preservar agora, executar depois

**Contexto.** Arquivos `.xlsm` dependem de macros. Executar VBA é um projeto grande e um risco de segurança (macros são um vetor clássico de malware).

**Decisão.**

- **v1**: o `vbaProject.bin` é **preservado intacto** (ADR-006). O app mostra que o arquivo tem macros, sem executá-las.
- **Futuro (`duck-vba`)**:
  1. Ler o container CFB (OLE) e descompactar o código-fonte (MS-OVBA).
  2. Parser e interpretador VBA em Rust.
  3. **Modelo de objetos estilo Excel** (`Application`, `Workbook`, `Worksheet`, `Range`, ...) implementado sobre a camada de comandos (ADR-009).
  4. Execução em **sandbox**: sem acesso a arquivos, rede ou processos por padrão; **desligado por padrão** e com confirmação por arquivo.
  5. Começar pelo subconjunto que planilhas de negócio usam (manipular ranges, loops, formatação, eventos simples); APIs de Windows (`Declare`, COM, ActiveX) ficam fora.
- **Caminho alternativo em estudo**: em vez de executar VBA, traduzir as macros uma única vez para a linguagem de script da duck-office ao converter para o formato nativo (ADR-013). O parser de VBA é o mesmo nos dois caminhos.

---

## ADR-011 · Plataformas

**Decisão.** **Windows e Linux (X11 e Wayland) desde o primeiro dia**; macOS depois. Nenhuma escolha pode impedir o macOS: todas as dependências da UI (`winit`, `wgpu`, `cosmic-text`, `rfd`, `arboard`) já suportam macOS (Metal).

- CI compila e testa em Windows e Linux em todo commit.
- Código específico de cada SO fica isolado em `duck-platform` (ADR-007); nenhum outro crate usa `#[cfg(windows)]` ou `#[cfg(target_os = "linux")]`.
- Distribuição inicial: binário único no Windows (instalador depois); no Linux, AppImage e/ou Flatpak.

---

## ADR-012 · Escalar com o hardware: paralelismo e SIMD

**Contexto.** Um PC bom de hoje tem 8 a 32 núcleos e unidades vetoriais (AVX2, AVX-512, NEON) que processam 4 a 16 números por instrução. Os editores atuais usam quase tudo isso em **uma thread, um número por vez**. A meta é: **quanto melhor a máquina, mais rápido abre e calcula**, sem deixar de funcionar bem num notebook simples.

**Decisão.** Paralelismo e vetorização são **requisitos de design desde o primeiro dia**, não otimizações para depois. Ficam centralizados no crate `duck-par`.

### Paralelismo: dados divididos em pedaços independentes

- **O bloco de 1024 linhas (ADR-004) é a unidade de trabalho.** Carga, busca, ordenação, filtro, cálculo e exportação são escritos como "aplicar em cada bloco" + "juntar os resultados". Isso escala naturalmente com o número de núcleos.
- **Escalonador com roubo de trabalho** (`rayon`): núcleos ociosos pegam trabalho dos ocupados, então o desempenho se ajusta sozinho a 4, 16 ou 64 núcleos.
- **Prioridades**: o que está na tela vem primeiro (aba ativa, área visível), o resto vai para background.
- **Uma thread sempre reservada para a UI**: em máquina fraca, o app fica um pouco mais lento para carregar, mas nunca trava.
- **Onde se aplica**:

| Operação | Como paraleliza |
|---|---|
| Abrir arquivo | partes do zip em paralelo; parse da aba gigante em pedaços por fronteira de `<row>` (ADR-005); inflate paralelo especulativo |
| Busca / substituir | cada bloco buscado num núcleo, com comparação SIMD |
| Ordenar / filtrar | ordenação paralela (radix/merge por blocos); filtro gera bitmap por bloco |
| Cálculo (v2) | grafo de dependências em **níveis**: tudo do mesmo nível roda em paralelo; fórmulas compartilhadas viram operação de coluna inteira (abaixo) |
| Salvar (v3) | cada aba serializada e compactada em paralelo (deflate de partes independentes) |
| Texto na tela | shaping e rasterização de glifos novos em paralelo antes do frame |

- **Resultado idêntico com qualquer número de threads.** Juntar resultados sempre em ordem fixa (por índice de bloco), nunca "quem terminar primeiro". Isso mantém os testes determinísticos e evita bugs que só aparecem em máquina com mais núcleos.
- **Sem contenção**: cada thread escreve nos próprios blocos e buffers; nada de lock por célula; estruturas por thread alinhadas em linha de cache (evita false sharing); alocador `mimalloc` com arenas por thread.

### SIMD: vários números por instrução

- **O formato colunar é o que torna o SIMD possível.** Números de uma coluna ficam contíguos em `f64`, prontos para AVX2 (4 por instrução) ou AVX-512 (8 por instrução).
- **Fórmula compartilhada = operação vetorial.** Uma fórmula como `=B2*C2` copiada para 500 mil linhas não é avaliada 500 mil vezes pelo interpretador: vira **um kernel SIMD sobre as colunas B e C inteiras**, em paralelo por blocos. Esse é o maior ganho esperado do motor de cálculo.
- **Agregações** (`SOMA`, `MÉDIA`, `CONT.SE`, `PROCV` com chave ordenada, mínimo/máximo) como kernels vetorizados por bloco.
- **Parse**: busca de `<`, `"` e fronteiras de linha com `memchr` (já usa SIMD); conversão de texto para número com algoritmos rápidos (`fast-float`); descompactação com `libdeflate`/`zlib-rs` (já vetorizados).
- **Detecção em tempo de execução**: o binário é compilado para uma base compatível com qualquer PC x86-64 (`x86-64-v2`) e, ao iniciar, escolhe a versão mais rápida de cada kernel que a CPU suporta (AVX2, AVX-512; NEON em ARM). Um único binário serve para máquinas velhas e novas. Ferramentas: `multiversion` / `pulp` ou `std::arch` com `is_x86_feature_detected!`.
- **Cuidado numérico**: somas vetorizadas mudam a ordem das operações em ponto flutuante. Onde o resultado precisa bater com o Excel, usamos somas compensadas (Kahan/Neumaier) ou ordem fixa, validadas no corpus de testes.

### Medir a escalabilidade

- `duck-bench` roda cada cenário com **1, 2, 4, 8, 16 threads** e com SIMD ligado/desligado, e publica a curva de speedup.
- Metas: speedup ≥ 5× com 8 threads na carga (seção 1); kernels SIMD ≥ 3× mais rápidos que a versão escalar.
- Regressão de performance acima de 10% em qualquer cenário falha o CI.

**Alternativas rejeitadas.**

- *Paralelizar depois que o código estiver pronto*: na prática exige reescrever estruturas de dados pensadas para uma thread. Por isso o bloco é a unidade desde o início.
- *Calcular na GPU*: interessante para volumes enormes, mas a transferência de dados e a compatibilidade numérica com o Excel não compensam para planilhas típicas. Fica como pesquisa futura.

**Consequências.** Todo código de dados precisa ser escrito "por bloco" e sem estado global mutável. Os kernels SIMD têm duas versões (vetorial e escalar de referência) e testes que comparam as duas.

---

## ADR-013 · Formato de arquivo nativo (futuro, em discussão)

> **Status: proposta aberta.** Não entra antes da F5. Esta seção registra a ideia e as perguntas para amadurecer; nada aqui está decidido.

**Contexto.** Um `.xlsx`/`.xlsm` é um **zip com dezenas a centenas de XMLs** (um por aba, estilos, strings, pivôs, gráficos), mais partes binárias (`vbaProject.bin`, Power Pivot). Para dados grandes, esse formato é ruim por natureza:

- **Tudo é texto.** O número `3.14159` é gravado como caracteres e precisa ser convertido de volta toda vez que o arquivo abre.
- **Precisa descompactar e fazer parse do XML inteiro** antes de usar qualquer coisa. A maior aba do arquivo de referência são 1,64 GB de texto para chegar a ~22M células.
- **Sem acesso direto.** Não dá para ler só a linha 500000: o deflate de cada parte é sequencial e o XML precisa ser percorrido do início.
- **Salvar reescreve tudo.** Mudar uma célula regrava e recompacta o arquivo inteiro.
- **Redundância**: `calcChain.xml` (228 MB no arquivo de referência) só guarda uma dica de ordem de cálculo.
- **Macros exigem um motor VBA** para funcionar.

O próprio Excel tem o `.xlsb` (binário), que resolve parte disso, mas continua sendo um zip com registros sequenciais por linha: sem acesso direto e sem layout colunar.

**Ideia.** Um formato próprio da duck-office (nome e extensão a definir, ex.: `.duck`), em que **o layout no disco é o mesmo layout da memória** (ADR-004). Abrir vira praticamente "mapear o arquivo", não "converter o arquivo".

- **Fluxo de uso**: abrir um `.xlsm` → o duck-sheets oferece converter para o formato nativo → as próximas aberturas são instantâneas. **Exportar de volta para `.xlsx`/`.xlsm` sempre funciona**, porque é o formato que as outras pessoas usam.

### Princípios em estudo

1. **Abrir = mapear.** Arquivo mapeado em memória (`mmap`), blocos de colunas gravados já no formato que o `duck-core` usa. Nada de parse para mostrar a primeira tela.
2. **Índice no cabeçalho.** Uma tabela de conteúdo diz onde está cada aba, cada coluna, cada bloco. Ler a linha 500000 é um salto direto.
3. **Compressão por bloco, independente.** Cada bloco compactado sozinho (LZ4 para velocidade, zstd para tamanho), então qualquer bloco pode ser lido isoladamente e **todos podem ser descompactados em paralelo** (ADR-012). Codificações por tipo de coluna, como em formatos analíticos: dicionário para texto repetido, delta e bit-packing para números e datas, run-length para estilos.
4. **Primeira tela primeiro.** Os blocos da área visível na última vez que o arquivo foi salvo ficam no início, para pintar antes de ler o resto.
5. **Fórmulas já compiladas.** Guardadas como AST/bytecode, com grupos de fórmulas compartilhadas e, opcionalmente, o grafo de dependências pronto. Recalcular não exige fazer parse de nenhuma fórmula.
6. **Valores calculados guardados**, como no xlsx, para abrir sem recalcular.
7. **Salvar incremental.** Só os blocos alterados são regravados (append + atualização do índice, com troca atômica). Editar uma célula num arquivo de 200 MB grava kilobytes, não 200 MB.
8. **Integridade.** Checksum por bloco; escrita segura contra queda de energia; um arquivo corrompido perde no máximo os blocos afetados, não tudo.
9. **Compatível com o futuro.** Cabeçalho com assinatura, versão e flags de recursos; seções desconhecidas podem ser puladas por versões antigas.
10. **Ida e volta sem perda.** Partes do xlsx que não convertemos (Power Pivot, web extensions, controles) vão como anexos opacos para a exportação de volta ao `.xlsm` sair igual à original (ADR-006).

### Macros sem motor VBA

Uma direção possível: **traduzir o VBA na conversão**, uma vez só, para a linguagem de script da própria duck-office (ou um bytecode dela), rodando na mesma API de comandos (ADR-009). O app não precisaria carregar um motor VBA para abrir o arquivo nativo.

- O VBA original fica guardado como anexo, para a exportação de volta ao `.xlsm`.
- Ponto honesto: **traduzir VBA automaticamente é difícil.** O subconjunto comum de planilhas (ranges, loops, formatação, eventos simples) é viável; chamadas a APIs do Windows, COM e ActiveX não. O conversor precisa listar o que não conseguiu traduzir.
- Isso se conecta com o ADR-010: o mesmo parser de VBA serve tanto para executar quanto para traduzir. Fica em aberto qual caminho seguir primeiro.

### Alternativas a avaliar antes de decidir

| Opção | A favor | Contra |
|---|---|---|
| **Formato 100% próprio** | Layout idêntico ao `duck-core`, controle total, salvar incremental do jeito que precisamos | Mais trabalho; ferramentas externas não leem |
| **Base em Apache Arrow (IPC) para as colunas** + container próprio | Formato colunar padrão, já pensado para `mmap` e SIMD; ferramentas de dados (Python, DuckDB, Polars) leriam as colunas direto | Arrow não modela estilos, fórmulas, merges; precisaria de seções extras |
| **SQLite como container** | Transacional, acesso direto, salvar incremental de graça, ferramentas por toda parte | Overhead por acesso, layout não é colunar, mais difícil de mapear direto para os blocos |
| **Adotar `.xlsb`** | Já é do Excel, lido por outras ferramentas | Continua zip + registros sequenciais; não resolve acesso direto nem salvar incremental |

Tendência inicial: **container próprio com codificação de colunas inspirada (ou compatível) com Arrow**. A decisão fica para quando o `duck-core` estiver estável e medido.

### Perguntas para amadurecer

- O formato nativo é **cache local** (o `.xlsm` continua sendo o original) ou **arquivo principal** (o usuário passa a trabalhar nele)? Talvez as duas coisas, em fases: primeiro cache/snapshot, depois arquivo de verdade.
- Como evitar que alguém envie um `.duck` para quem só tem Excel? (Exportação automática, aviso ao compartilhar, extensão clara.)
- Qual linguagem de script recebe o VBA traduzido? (Uma linguagem própria simples, Lua, Rhai, ou um bytecode nosso.)
- Gráficos, pivôs e slicers: guardar só a definição e recalcular, ou guardar também o resultado pronto?
- Colaboração/versões: o formato guarda histórico de alterações (útil com o salvar incremental)?

**Consequências (se adotado).** O tamanho em disco tende a cair (colunas comprimem muito melhor que XML), a abertura de arquivos gigantes passa a ser medida em milissegundos, e salvar deixa de ser proporcional ao tamanho do arquivo. O custo é manter um formato e um conversor bidirecional estáveis por muitos anos.

---

## Roadmap

| Fase | Entrega | Critério de pronto |
|---|---|---|
| **F0** | Esqueleto do workspace; primeira versão de `platform/` (`duck-platform`, `duck-gfx`, `duck-text`) com o exemplo `million_rows`: **grade falsa de 1M linhas** com texto, sem nenhum crate de office | 60+ fps rolando em Windows e Linux; regra de dependência verificada no CI |
| **F1** | `duck-par` + `duck-xlsx` lendo valores, strings e estilos com **parse paralelo em pedaços**; `duck-bench` medindo tempo, RAM e curva de speedup (1→16 threads); gerador sintético | Números reais do arquivo de referência; metas revisadas |
| **F2** | **Visualizador completo** (v1): abas, estilos, merges, larguras, congelar painéis, busca paralela/SIMD, copiar, carga progressiva (a leitura de layout e o inflate paralelo já estão prontos) | Abre o arquivo de referência dentro das metas |
| **F3** | Edição + escrita com round-trip (salvar em paralelo) | Teste de round-trip verde no corpus |
| **F4** | Motor de cálculo: grafo de dependências por intervalos, recálculo incremental em níveis paralelos, fórmulas compartilhadas como kernels SIMD | Recalcula o arquivo de referência sem travar a UI |
| **F5+** | Gráficos, pivôs interativos, slicers, VBA, formato nativo (ADR-013), `duck-docs`, `duck-slides` | — |

### Status da F0 (18/11/2026)

Entregue em `platform/`: `duck-platform`, `duck-gfx`, `duck-text` e o exemplo `million_rows` (1.000.000 de linhas × 40 colunas, alturas variáveis, seleção, teclado, roda do mouse, scrollbars arrastáveis, clipboard). `duck-ui` (widgets) ficou para quando houvesse o primeiro widget de verdade: entrou na F2.

Medido com `million_rows --bench` (mesma máquina: Radeon RX 9070 XT, janela 1440×900, vsync desligado):

| Métrica | Windows 11 (DX12, GPU) | Linux/WSLg Wayland (Vulkan **por software**) | Linux/WSLg X11 (Vulkan **por software**) | Meta |
|---|---|---|---|---|
| Primeiro frame na tela (processo inteiro) | **257–286 ms** | 319 ms | 120 ms | < 300 ms |
| CPU por frame, scroll vertical (p50 / p99) | 0,74–0,80 / 1,5 ms | 3,9 / 4,8 ms | 4,3 / 5,7 ms | < 16 ms |
| CPU por frame, saltos aleatórios (p50 / p99) | 0,77–0,83 / 1,4 ms | 4,0 / 4,9 ms | 4,4 / 5,7 ms | < 16 ms |
| Frames por segundo sem vsync | ~1.150–1.250 | ~250 | ~180 | 60+ |
| Draw calls por frame | 1 | 1 | 1 | 1–3 |

No Linux, o WSL desta máquina não expõe a GPU (só o llvmpipe, que renderiza na CPU, tanto em Vulkan quanto em OpenGL). Mesmo assim a grade fica acima de 180 fps, o que mostra que o desenho por instâncias aguenta até sem placa de vídeo. A renderização no Linux (fonte DejaVu Sans, acentos, números, `###`) foi conferida por screenshot.

O que a F0 ensinou:

- **Inicializar todos os backends gráficos custa ~900 ms**; um só, ~250 ms. Por isso o `duck-gfx` abre só o backend nativo do SO e procura nos outros apenas se ele falhar **ou só oferecer renderização por software** (ex.: Linux com lavapipe instalado mas sem driver Vulkan para a placa, e OpenGL acelerado disponível). Sem GPU em lugar nenhum, fica com o software do backend nativo. No Windows, o DX12 também rendeu frames ~30% mais rápidos que o Vulkan.
- **OpenGL por software trava em 60 fps** mesmo com vsync desligado; o Vulkan por software não. Mais um motivo para o Vulkan ser o principal no Linux.
- **Sondar GPU sem tocar na janela** (19/11/2026). A busca por GPU em outros backends criava e destruía superfícies Vulkan e EGL na mesma janela. Numa das execuções, o compositor do WSLg (Weston) caiu com segfault (`/mnt/wslg/stderr.log`: `terminated with signal 11`), e o programa morreu com `Broken pipe`. Agora a sondagem usa uma instância sem display (não conecta ao compositor) e só pergunta se há adaptador de hardware; a janela ganha uma segunda superfície apenas se houver. No WSL: 10 aberturas seguidas sem queda, e o primeiro frame caiu de ~300 para ~160 ms.
- **GPU no WSL**: o WSL expõe a placa como `/dev/dri/renderD128`, do grupo `render`. Um usuário fora desse grupo fica com renderização por software (llvmpipe), sem aviso além de um `Permission denied` do Mesa.
- **Fontes carregam em paralelo** com a janela e a GPU (o `TextSystem` é `Send`); a espera por elas sumiu da abertura.
- **Caminho rápido para números** no `duck-text`: dos ~3,6 milhões de textos desenhados no benchmark, ~1,6 milhão foram números montados por caractere, sem shaping, e só ~2 mil strings precisaram de shaping completo.
- **Número que não cabe na célula vira `###`**, como no Excel: cortado, ele mostraria um valor errado.
- O `duck-gfx` também renderiza fora da tela (`render_to_rgba`, usado por `million_rows --screenshot`), base para testes visuais automáticos.

**Grade com 10 milhões de linhas** (19/11/2026, `million_rows --bench --rows 10000000`):

| | 1 milhão | 10 milhões |
|---|---|---|
| Primeiro frame na tela (Windows, DX12) | 217–247 ms | 248–273 ms |
| Índice das linhas (Fenwick) | 4 ms | 43–46 ms (Linux: 73 ms) |
| CPU por frame, scroll / saltos aleatórios (p50, Windows) | 0,81 / 0,84 ms | 0,81 / 0,85–0,88 ms |
| CPU por frame, scroll (p50, Linux por software) | 4,1 ms | 4,1 ms |
| Memória do processo (Windows / Linux) | 190 / 178 MB | 294 / 277 MB |

O custo por frame não muda com 10× mais linhas: só as células visíveis são processadas e achar a linha num pixel é O(log n). O que cresce é o índice: ~10 bytes por linha (tamanho `u32` + árvore `i64`), ou ~105 MB e ~40 ms para 10 milhões. Na F2, o índice deve guardar só as linhas com altura própria (no arquivo de referência são 15 mil de 593 mil) sobre uma altura padrão, em vez de uma entrada por linha.

Pendências da F0:

- **Linux com GPU de verdade.** No Linux só foi testado o WSLg, com renderização por software. Falta um Linux nativo (ou WSL com driver de GPU) para medir o caminho acelerado.
- **Teclado e mouse no Linux** não foram testados por mim (só a renderização). Teste manual: `cargo run --release -p million_rows` no WSL, com setas, Ctrl+G, roda do mouse e arrastar as scrollbars.
- **Memória base de ~200 MB** com a janela aberta (boa parte é o driver da GPU). Medir na F2, quando a janela e os dados estiverem no mesmo processo.
- O CI (`.github/workflows/ci.yml`) está escrito, mas ainda não rodou: o repositório git é só local, sem remoto no GitHub.

### Status da F1 (18/11/2026)

Entregue em `crates/`:

- **`duck-core`**: armazenamento colunar em blocos de 1024 linhas (11 bytes por célula: tipo, estilo `u16`, payload de 64 bits; compactados durante a leitura desde a F2), tabelas de strings contíguas, estilos (`cellXfs`, fontes, preenchimentos, formatos de número) e montagem de abas por partes (`SheetPart` + `merge_part` em ordem fixa).
- **`duck-par`**: pool com número de threads escolhido, limitador de pedaços em memória que executa trabalho enquanto espera (não trava com 1 thread) e corte de buffers por busca SIMD.
- **`duck-xlsx`**: zip mapeado em memória; strings compartilhadas lidas em paralelo por fronteiras de `</si>`; estilos; todas as abas ao mesmo tempo (maiores primeiro); abas grandes em pedaços de 8 MB cortados depois de `</row>`, cada um numa thread; aba sem número de linha relida em sequência.
- **`duck-bench`**: `open`, `sweep` (um processo por rodada), `verify` (compara cada célula com o `calamine`, um leitor independente, sem expor o conteúdo das células no relatório) e `gen` (planilha sintética com o perfil da referência).

Resultado no arquivo de referência (185 MB no disco, 1,9 GB de XML, 20 abas):

| | Windows 11 | Linux (WSL, Ubuntu 26.04) |
|---|---|---|
| Células com valor / com fórmula | 24.171.714 / 10.111.650 | igual |
| Divergências contra o calamine | **0** | **0** |
| Leitura completa, 8 threads | 650 ms | 648 ms |
| Leitura completa, 1 thread | 3.950 ms | 3.971 ms |
| Ganho com 8 / 16 threads | 6,07× / 6,41× | 6,13× / 6,41× |
| Memória com tudo carregado | ~460 MB (dados: 345 MB) | ~490 MB |
| Pico durante a leitura | 0,87–0,96 GB | 0,95–1,1 GB |

O que a F1 ensinou:

- **O gargalo agora é o inflate da aba gigante**: 1,56 GB de XML num único stream deflate leva ~0,8 s numa thread só, e a curva de ganho satura em ~6,4× a partir de 8 threads. O parse em si escala bem (~400 MB/s por thread). O **inflate paralelo especulativo** é a prioridade de performance da F2.
- **Fim de linha do XML**: a especificação manda converter `\r\n` e `\r` literais em `\n`; só um CR escapado (`_x000D_`, `&#13;`) sobrevive. O `verify` achou 357 células com quebra de linha erradas no arquivo real antes da correção. Sem a comparação célula a célula, isso teria passado.
- **Alocador**: o `mimalloc` deixou a leitura ~20% mais rápida que o alocador do sistema, mas segura a memória dos pedaços liberados em cada thread. Chamar `mi_collect` em todas as threads do pool ao fim da leitura traz a memória de ~850 MB de volta para ~460 MB. O pico (~1 GB) ainda vem dessa retenção durante a leitura.
- O limite de pedaços em memória (8 a 66) quase não muda o tempo nem o pico neste arquivo.

### Pendências da F1 resolvidas (18/11/2026)

As três pendências da F1 foram resolvidas:

**1. Inflate paralelo** (crate novo `duck-deflate`)

- Decodificador deflate próprio, porque nenhuma biblioteca decodifica a partir do meio de um stream. Numa thread ele faz ~1,7 GB/s, cerca de 80% do zlib-rs.
- Em paralelo, a aba gigante descompacta em **~200–230 ms**, 3–3,6× o zlib-rs. A saída tem bytes idênticos aos do zlib-rs, e o CRC32 do zip é conferido sempre.
- A curva de ganho não satura mais em ~6,4×: chega a 9,2× com 32 threads.
- A saída é entregue por posse e lida sem cópia: cada tarefa de parse lê um intervalo de um vetor compartilhado.
- O merge das partes acontece em fluxo, na ordem, assim que cada parte fica pronta.

**2. Pico de memória abaixo de 1 GB** nos dois sistemas e com qualquer número de threads (563–936 MB). Os limites que fazem isso:

- 8 partes especuladas à frente (cada uma tem 1 MB comprimido);
- até 8 vetores descompactados em trânsito no parse;
- buffers reciclados num pool;
- páginas já lidas do arquivo mapeado devolvidas ao sistema no Linux (`MADV_DONTNEED`);
- duas opções do `mimalloc` (abaixo).

**3. Fórmulas e layout**

- **Texto de todas as fórmulas** (crate novo `duck-calc`):
  - As compartilhadas são remontadas deslocando as referências relativas da célula-mestre.
  - As normais viram **modelos relativos deduplicados**: no arquivo de referência, 6,5 milhões de fórmulas viraram 11 modelos.
  - Fórmulas que não dependem da posição (colunas calculadas de tabela, só referências absolutas) são reaproveitadas por coluna sem nem serem decodificadas.
- **Layout**: larguras de coluna, alturas e linhas ocultas, células mescladas, painéis congelados, altura e largura padrão e `<dimension>`.

**Conferência contra o calamine**, no arquivo de referência, em Windows e Linux, com o inflate paralelo e com 1 thread:

| | Comparados | Divergências |
|---|---|---|
| Células | 24.171.714 | **0** |
| Fórmulas (texto) | 10.111.650 | **0** |
| Mesclagens | 29 | **0** |

O que esse trabalho ensinou:

- **Blocos vazios de flush**: o compressor do Office insere blocos sem compressão vazios (`00 00 FF FF`) entre os blocos dinâmicos. A especulação começava no bloco seguinte, ~80 bits depois de onde a parte anterior terminava. Pular esses blocos levou o aproveitamento de 91 para 104 das 105 partes.
- **Colunas calculadas de tabela**: 4,75 milhões de fórmulas não tinham nenhuma referência de célula (só `[#This Row]`). Guardar o texto de cada uma levou os dados a 1,76 GB; deduplicadas, voltaram a 392 MB.
- **`ca="1"`**: fórmulas com `TODAY()` têm esse atributo, o que desligava o cache por coluna. Generalizar o cache para "qualquer fórmula que não dependa da posição" levou o parse de 7,4 s para 3,3 s de CPU.
- **Memória do inflate paralelo**: o limite certo é por *vetor* descompactado, não por pedaço de parse. Um pedaço pequeno segura o vetor inteiro (~15 MB) de onde saiu.
- **`mimalloc`**:
  - Com o padrão (`purge_delay` de 10 ms), o pico passava de 1,5 GB. Com 1 ms fica abaixo de 1 GB.
  - No Linux, `arena_eager_commit` compromete a arena inteira de saída e inflava o RSS em ~250 MB.
  - Essa opção precisa ser definida **antes** do runtime do Rust (construtor em `.init_array`), porque ele já aloca antes do `main`.

Pendências:

- ~~**Parse mais barato**~~: feito (ver abaixo). As metas de < 0,4 s e ≥ 10× com 16+ núcleos ainda não foram cumpridas.
- **Opções do alocador no app**: as opções do `mimalloc` hoje ficam no `duck-bench`. O app (`duck-sheets`) precisa aplicar as mesmas.

### Otimização do parse (19/11/2026)

Medido com `duck-bench parse` (só o parse da aba gigante, numa thread, sobre o XML já em memória) e `duck-bench open` (que agora mostra também o tempo de CPU):

| Mudança | Efeito |
|---|---|
| Célula lida **numa passada só**: depois da tag, cada `<` decide se é `<v>`, `<f>`, `<is>` ou `</c>`; o texto de `<v>`/`<f>` vai até o próximo `<` | parse 3,55 → 1,98 s (176 → 98 ns por célula) |
| Busca de `<`/`>` híbrida: laço simples nos primeiros 32 bytes, `memchr` só depois (as distâncias numa célula são de 0 a ~20 bytes) | 1,98 → 1,82 s |
| Atributos da célula lidos byte a byte, sem `memchr` nem `trim` | 1,82 → **1,67 s (83 ns por célula)** |
| Merge das partes com `try_lock` (quem chega com o merge ocupado só deixa a parte na fila) | workers deixam de ficar bloqueados |
| Quem alimenta o inflate paralelo espera vaga **sem pegar tarefas** do pool, dormindo (`park`) até ser acordado | a entrega não para mais rodando um parse de outro |
| Marcadores resolvidos **no pool** com a janela publicada pela parte anterior (102 de 105 partes) | resolução serial: ~290 → ~5 ms |
| Decodificador: escrita direta por ponteiro, cópias em blocos de 16, consulta de tabela sem checagem de limites, caminho rápido também na fase de 16 bits, CRC calculado bloco a bloco (bytes ainda no cache) | inflate paralelo isolado: 264 → ~200 ms, CPU 1,70 → 1,20 s |

O que essa rodada ensinou:

- **Medir o parse isolado** mostrou que só ler o XML, sem guardar nada, custava 76 ns por célula: o gargalo era a varredura (várias buscas curtas por célula), não o armazenamento.
- **Livelock**: a espera "sem ajudar o pool" posta em todos os leitores de aba travou com 8 threads e 20 abas. Todos os workers ficavam esperando vagas que só as tarefas paradas nas filas deles liberariam. Ela vale só para o consumidor do inflate paralelo, que é único.
- **O trabalho total cresce com as threads** (aba gigante: 2,3 s de CPU com 1 thread, ~6,2 s com 16, ~7 s com 32). Parte é o inflate especulativo (a fase de 16 bits e a decodificação custam mais que o zlib-rs), parte é SMT e o clock menor com todos os núcleos ligados.
- **SMT não ajuda** neste trabalho: 32 threads empatam com 16 e gastam mais memória.

**Inflate e parse na mesma tarefa** (`duck_deflate::inflate_parallel_process`):

- Cada parte especulada é lida pela própria thread que a descompactou, com os bytes ainda no cache, mas só as linhas inteiras do meio dela.
- O começo da parte (que depende da janela anterior) e as linhas que atravessam a fronteira com a vizinha são costurados pela thread consumidora.
- Essa thread passou a só validar a ordem e encaminhar as partes já lidas ao merge.
- **0 divergências** contra o calamine, como antes.

Resultado: **não deu o salto esperado**. Comparando as duas versões alternadas na mesma sessão, a fundida ganhou ~4% (mediana 630 × 660 ms com a máquina ocupada), e ficou por isso. Partes maiores (2 e 4 MB) foram piores: menos paralelismo e mais memória.

Onde isso para:

- Com 16 threads a leitura usa ~6–7 s de CPU e mantém ~13–14 núcleos ocupados em média (~85%). Está **limitada pela CPU total**, não por um trecho serial.
- Com 1 thread o mesmo trabalho custa 3,4 s. A diferença vem do inflate especulativo (~1,2 s de CPU contra 0,7 s do zlib-rs), do clock menor com todos os núcleos ligados e da disputa por cache e memória.
- Os números variam de 450 a 560 ms com 16 threads conforme a carga dos outros programas da máquina.
- Para chegar a < 0,4 s, falta cortar ~15–20% da CPU total. As alavancas que sobram são micro-otimizações do parse (hoje 83 ns por célula numa thread) e do decodificador (hoje ~1,6 GB/s numa thread contra ~2,2 GB/s do zlib-rs), com retorno incerto.
- ~~**Bordas** são só contadas~~: lidas por lado na F2 (ver abaixo).
- ~~**Opções do alocador no app**~~: o `duck-sheets` aplica as mesmas (F2).

### Status da F2 (19/11/2026)

Entregue: o app **`apps/duck-sheets`** abre `.xlsx`/`.xlsm` numa janela nativa, com carga progressiva e a aparência do Excel.

**Leitura (`duck-core`, `duck-xlsx`)**

- **Formatos de número** (`duck_core::numfmt`), com saída em pt-BR (vírgula decimal, ponto de milhar, meses e dias em português):
  - seções (positivo; negativo; zero; texto), cores (`[Red]`, `[Color10]`) e condições (`[>=100]`);
  - milhar, escala (`#,##0,`), %, científico (inclusive de engenharia), frações;
  - datas e horas (inclusive o 29/02/1900 fictício do Excel, base 1904, tempo decorrido `[h]` e frações de segundo);
  - moeda (`[$R$-416]`), literais, `_` e o preenchimento `*` dos formatos contábeis ("R$" à esquerda, número à direita);
  - formatos internos 0–49 já em pt-BR (14 = `dd/mm/aaaa`);
  - "Geral" com até 11 caracteres, perdendo casas decimais para caber na coluna.
- **Tema** (`theme1.xml`: 12 cores e fontes), **tonalidade** (`tint`, em HLS como o Excel), paleta indexada (inclusive a própria do arquivo).
- **Bordas** por lado (13 estilos: fina, média, grossa, dupla, pontilhada, tracejada...).
- **Visão da aba**: linhas de grade, zoom, célula do canto, célula ativa, cor da guia e aba ativa do workbook.
- **API em etapas** (`duck_xlsx::Book`): abrir, ler estilos+tema, strings, uma **prévia** da aba (os primeiros MB de XML) e abas inteiras com entrega uma a uma. O `open()` de sempre passou a usar as mesmas peças.

**Plataforma (`platform/`)**

- `duck-platform`: diálogo nativo de abrir arquivo e de mensagem (`rfd`), arquivo arrastado para a janela, modificadores e contagem de cliques no mouse, e um `Waker` para outras threads acordarem a janela.
- `duck-gfx`: instância **tracejada** (o padrão é feito no shader, então uma borda pontilhada é um retângulo só).
- `duck-text`: **estilos** (família, tamanho, negrito, itálico) com cache por estilo, equivalentes métricos quando a fonte não existe (Calibri → Carlito, Arial → Liberation Sans...) e **quebra de linha**.
- **`duck-ui`** (crate novo): `Painter` (retângulos, contornos e texto alinhado com recorte), `TextInput` (cursor, seleção, colar/copiar, acentos) e `TabBar` (abas com cor e rolagem).

**O app**

- Abre pela linha de comando, pelo diálogo (Ctrl+O ou sem argumento) ou arrastando o arquivo.
- **Carga progressiva**: prévia da aba ativa → primeira tela → aba ativa inteira → outras abas, cada uma entregue quando fica pronta. Abas ainda carregando aparecem com "…" na guia.
- Larguras de coluna em pixels pela fórmula do Excel (largura do dígito da fonte padrão), alturas de linha, linhas e colunas ocultas, estilo de coluna para células sem `<c>`.
- **Índice esparso de linhas**: altura padrão + só as linhas com altura própria (15 mil de 593 mil na aba gigante), com busca binária, em vez de uma entrada por linha (pendência da F0).
- Fontes, cores, preenchimentos (sólidos e hachuras), alinhamentos (inclusive recuo), texto quebrado, sublinhado, bordas, `###` e "Geral" que encolhe.
- **Texto que transborda** para as vizinhas vazias (à direita, à esquerda ou dos dois lados, conforme o alinhamento), apagando as linhas de grade por baixo, inclusive vindo de colunas fora da tela.
- Mesclagens, **painéis congelados** (quatro quadrantes), abas (ocultas ficam de fora), barra de fórmulas (fórmula como está no arquivo ou o valor guardado) e caixa de nome ("ir para", Ctrl+G).
- Seleção com mouse, Shift e cabeçalhos; Ctrl+setas até a borda dos dados; **Ctrl+C** copia o que as células mostram, em TSV como o Excel; soma, média e contagem da seleção na barra de status.
- **Busca** (Ctrl+F, F3) em segundo plano e em paralelo, sem diferença de maiúsculas nem de acentos; resultados destacados na grade.
- **Zoom** (Ctrl + roda, Ctrl +/−/0), começando no zoom salvo na aba.
- `mimalloc` com as mesmas opções do `duck-bench`.
- Modos de medição: `--bench` (rola a aba e mede) e `--screenshot` (renderiza fora da tela e sai), mais `--sheet`, `--cell`, `--zoom` e `--find`.

**Medido** no arquivo de referência (1440×900). Windows = AMD Radeon RX 9070 XT em DX12, aba ativa pequena, 3 rodadas em 21/11/2026; Linux = WSLg com renderização **por software** (llvmpipe):

| | Windows (GPU) | Linux (WSLg, software) | Meta |
|---|---|---|---|
| Pacote + estilos + strings + prévia da aba ativa | 15 ms (`duck-bench preview`) | — | — |
| Arquivo aberto + prévia (processo inteiro) | 68–70 ms | — | — |
| Abertura da GPU (instância + adaptador / device) | 104–112 / 42–43 ms, numa thread desde o início do processo; a janela espera 91–104 ms por ela | — | — |
| **Primeira tela com dados** (processo inteiro) | **190–199 ms** (era 212–245) | **127–182 ms** (aba pequena ou a de 593 mil linhas) | < 300 ms |
| Aba ativa inteira | 0,22 s (aba pequena) | 0,18 s (aba pequena) / 0,74 s (593 mil linhas) | — |
| Todas as 20 abas | 0,61–0,65 s | 0,75–0,95 s | — |
| Montagem do frame, p50 / p99 (scroll, saltos, horizontal) | 0,5–0,6 / 0,6–2,5 ms | 1,5–2,0 / 2,3–3,2 ms | < 16 ms |
| Frame inteiro, p99 / pior | 1,4–4,7 / 9,1 ms | — | < 16 ms |
| Frames por segundo sem vsync | 866–1.147 | 74–455 (a CPU também rasteriza) | 60+ |
| Busca de um texto presente em 593.557 células | — | 33 ms | — |
| Memória com tudo carregado / pico | **423–437 / 517–527 MB** (privada: 390–403 MB) | 401–412 / 475–489 MB (privada: 282–294 MB) | < 600 MB |

A conferência contra o calamine continua com **0 divergências** (24,2 milhões de células, 10,1 milhões de fórmulas, 29 mesclagens). Uma planilha de vitrine sem dados reais (`duck-bench gen-showcase`) cobre formatos, bordas, painéis e abas, e entrou no CI.

O que a F2 ensinou:

- **A carga completa disputava a CPU com a própria janela.** Começando junto com a prévia, ela ocupava todos os núcleos enquanto o compositor mostrava a janela, e o primeiro frame saía em 550–900 ms. Agora a etapa 2 espera a primeira tela com dados ser apresentada (ou no máximo 400 ms): **162–182 ms** em todas as abas testadas.
- **Trabalho da janela no pool global espera na fila da leitura.** As estatísticas da seleção usavam `par_iter` no pool global, e a janela ficava parada até a leitura liberar threads. Trabalho pedido pela janela usa um pool próprio.
- **A prévia é barata**: 4 MB de XML (centenas a milhares de linhas) são lidos em ~2 ms, e com estilos e strings ficam prontos em 15 ms.
- **Strings compartilhadas na busca**: as ~40 mil strings são testadas uma vez; as células são reconhecidas pelo endereço do texto dentro da tabela (`StringTable::byte_range`), sem guardar o índice.
- **Formatos contábeis** precisam do preenchimento `*` para parecerem os do Excel; sem ele, "R$" e o número ficam colados.
- **Varrer as fontes do sistema atrasava a primeira tela no Windows.** O `FontSystem::new()` do cosmic-text lê as ~350 fontes instaladas: ~170 ms com o cache de disco quente e **~600 ms com ele frio** (primeira rodada depois de compilar: primeira tela em 651 ms). E a leitura da planilha só começava depois das fontes e da GPU. Agora o `duck-text` começa com 12 fontes essenciais (`TextSystem::new_fast`: Segoe UI, Calibri e Arial), o resto é lido numa thread depois da primeira tela e entra com `TextSystem::add_fonts` sem invalidar o atlas (as faces mantêm o id), e a leitura da planilha começa junto com a abertura da GPU. O caminho crítico ficou sendo só a GPU (~200 ms).
- **A memória no Windows passava da meta (655 MB) e quase toda era do modelo.** Medida separando a privada da compartilhada: o driver da placa ocupa ~135 MB privados com qualquer arquivo (DX12 e Vulkan parecidos), os dados ~396 MB e o resto era sobra do alocador. Nos 25 mil blocos de 11 KB do arquivo de referência, 83% têm um estilo só, 17% não têm valor (só estilo), 72% têm valores que cabem em 32 bits (inteiros, índices de string) e 70% dos blocos de fórmula repetem o mesmo id. Agora `Sheet::finish` compacta cada bloco (estilo único vira um valor; payload de 0, 32 ou 64 bits; ids de fórmula iguais viram um só), em paralelo por coluna: **dados de 396 para 198 MB**, processo de 655 para ~450 MB, leitura com o mesmo tempo e 0 divergências. Escrever num bloco compactado o descompacta (base para a edição da F3). Percorrer todas as células ficou 2,4× mais rápido (126 → 53 ms), porque `for_each_value` decodifica direto do bloco em vez de procurá-lo de novo a cada célula. E, no fim da leitura, o `mi_collect` roda em cada thread do pool: as páginas livres ficavam presas nas threads paradas (~30 MB).
- **O pico da leitura (~960 MB no app, 820–860 MB no `duck-bench`) tinha três causas**, todas na aba de 1,6 GB de XML:
  - *A aba só era compactada no fim* (~300 MB sem compactar). Agora cada thread compacta os blocos que ficam inteiros dentro da própria parte (`SheetPart::pack_inner`), e o merge, que recebe as partes na ordem do arquivo, compacta os da fronteira assim que a parte seguinte chega. Se as linhas vierem fora de ordem, escrever num bloco compactado o descompacta: fica mais lento, nunca errado. As strings locais compactadas passaram a aceitar 4.096 tabelas, uma por parte (antes eram 256; a aba gigante tem mais de 400 partes).
  - *O arquivo mapeado ficava inteiro na memória do processo no Windows* (~150 MB compactados da aba). O `Package::release` já tirava as páginas lidas no Linux (`MADV_DONTNEED`); no Windows agora usa `VirtualUnlock`, que tira do working set páginas que não estão travadas (elas continuam no cache de arquivos do sistema).
  - *Cada tarefa do inflate paralelo segurava a parte inteira descompactada* (~15 MB × 16 tarefas). Agora `inflate_parallel_process` entrega a saída em pedaços de 1 MB, guardando só a janela de 32 KB entre eles, e a leitura das linhas emenda os pedaços (até um `</row>` cortado ao meio).
  - Resultado no `duck-bench open` (16 threads): **pico de 820–860 para 352–373 MB** e leitura de 465–480 para **369–419 ms** (pedaços de 1 MB cabem no cache, o que deixou o parse mais rápido). Com 4 e 8 threads o pico também cai à metade e a leitura fica 7–10% mais rápida; no Linux, 493–535 para 321–338 MB e 568–651 para 489–497 ms. No app: pico de 954–979 para 517–527 MB no Windows e de 652–658 para 475–489 MB no Linux, 0 divergências nos dois.

**Fechamento da F2 (22/11/2026)**

- **Estilo de linha inteira** (`<row s customFormat="1">`): vale para as células sem `<c>` da linha, antes do estilo da coluna (`View::blank_style`), como no Excel.
- **Texto girado** (`textRotation`): 0–180° e letras empilhadas (`255`). O `duck-gfx` ganhou um glifo girado (`KIND_GLYPH_ROT`: ângulo nos bits altos de `kind`, rotação no vertex shader e interpolação bilinear à mão no fragment, só dentro do glifo). O renderer não recorta glifos girados: `Painter::text_rotated` descarta os que têm o centro fora da célula. Exigir o glifo inteiro dentro fazia sumir letras acentuadas em células justas.
- **Texto rico**: os trechos `<r><rPr>` das strings compartilhadas ficam ao lado da tabela (`StringTable::rich`, só para as strings que têm), e a célula reconhece a string pelo endereço do texto, como na busca. Cada trecho tem fonte, tamanho, cor, negrito, itálico, sublinhado, tachado e sobrescrito/subscrito, numa linha ou com quebra por palavra entre trechos. Tachado também nas fontes de célula. Texto inline (`<is>`) com trechos continua simples.
- **Formatação condicional** (`duck_core::cond`, leitura em `duck-xlsx/src/cond.rs`, `dxfs` no `styles.xml`):
  - tipos `cellIs`, `expression`, `colorScale` (2 e 3 cores), `dataBar`, `iconSet` (como caracteres coloridos), `top10`, `aboveAverage`, `duplicateValues`/`uniqueValues`, textos (contém, não contém, começa, termina), vazias, erros e períodos de data;
  - prioridade e `stopIfTrue` como no Excel: em cada propriedade (preenchimento, cor, negrito, bordas...) vale a regra de maior prioridade que a define;
  - fórmulas das regras passam por um avaliador pequeno (operadores, referências relativas à primeira célula do intervalo, `AND`/`OR`/`NOT`/`IF`/`IS*`/`LEN`/`MOD`/`ROW`/`TODAY`...). Regras com outras abas, intervalos (`COUNTIF`) ou funções fora da lista não se aplicam, sem afetar o resto;
  - o que depende do intervalo inteiro (mínimo, máximo, percentis, média, N maiores, contagem de repetidos) é calculado uma vez por regra, na primeira vez que ela aparece na tela (`CfState`), e cada frame avalia só as células visíveis;
  - fora por enquanto: regras da extensão de 2010 (`x14`, no `extLst`: 12 no arquivo de referência) e o formato de número do `dxf`.
- **Hiperlinks**: `<hyperlink>` da aba, com o `r:id` resolvido nos relacionamentos dela. Com o mouse em cima, cursor de link e o destino na barra de status; **Ctrl+clique** segue o link: lugar no arquivo (`Aba!A1`) muda de aba e seleciona, e endereço externo abre no programa padrão, mas só `http`, `https` e `mailto`, porque o endereço vem do arquivo.
- **Redimensionar com o mouse**: arrastar a borda do cabeçalho muda a largura da coluna ou a altura da linha (`Axis::set_size`, que refaz as somas só a partir do item mudado). Duplo clique na borda ajusta a coluna ao texto mais largo (até 20 mil células) e a linha volta à altura padrão. Os tamanhos mudados sobrevivem quando a aba é remontada (prévia → aba inteira, fontes novas).
- **GPU antes da janela**: no DX12, `Gfx::start_early` abre instância, adaptador e device numa thread desde o início do processo, e `Gfx::new_with` só cria a superfície quando a janela existe. Se o adaptador não servir para a janela, abre do jeito normal. No Linux fica como estava, porque a escolha do adaptador depende da superfície (Vulkan × software no WSLg). **Primeira tela de 212–245 para 190–199 ms.**
- **`.csv`** (e `.tsv`/`.txt`): uma aba só. Separador detectado entre `;`, `,`, tab e `|`; com `;`, vírgula decimal e ponto de milhar, como o Excel em pt-BR; UTF-8 com ou sem BOM, senão Windows-1252. Datas ficam como texto.
- A vitrine ganhou a aba "Recursos" com tudo isso, inventado, para conferir sem dados reais.

Medido depois (Windows, mesmo arquivo): leitura completa no `duck-bench open` em 378–389 ms com pico de ~355 MB (igual a antes), 864–1.091 fps sem vsync, 422–425 MB com tudo carregado e pico de 519–520 MB; 0 divergências no `verify`. No Linux: primeira tela em 127–142 ms, 408 MB e pico de 467–484 MB.

Pendências da F2:

- **Teste manual de teclado e mouse** na janela. Seleção, Ctrl+setas, busca, cópia e referências têm teste de código; redimensionar e Ctrl+clique em link ainda não foram usados à mão.
- Gráficos, imagens e slicers ficam para a F5.

---

## Questões em aberto

- **Licença** do projeto (MIT/Apache-2.0, GPL, outra).
- **Nome próprio** para a biblioteca de `platform/`, já que ela vai ser usada fora da suíte (por enquanto os crates usam o prefixo `duck-`).
- **Formato de arquivo nativo**: ideia registrada no ADR-013, com as perguntas que precisam de resposta antes de decidir.
- **Corpus de testes**: reunir planilhas públicas grandes e variadas além do gerador sintético.
- **Motor de cálculo**: avaliar compatibilidade numérica com o Excel (datas, precisão, funções financeiras) antes da F4.
