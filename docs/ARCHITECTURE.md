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
│   ├── duck-core          # modelo do workbook: células, blocos, strings, estilos
│   ├── duck-xlsx          # leitura (v1) e escrita (v2) de xlsx/xlsm em streaming
│   ├── duck-calc          # fórmulas: parser, AST, grafo de dependências (avaliação na v2)
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
   - O inflate de uma parte é sequencial por natureza; ele vira o limite. Para quebrar esse limite, o **inflate paralelo especulativo** (técnica do rapidgzip) entra como otimização prioritária na F2 (ADR-012).

3. **Partes independentes em paralelo**: `sharedStrings`, `styles` e todas as abas carregam ao mesmo tempo (as células só referenciam ids, então não há dependência de ordem). **A aba ativa tem prioridade** no escalonador. A grade é pintada assim que as primeiras linhas chegam (**carga progressiva**); o resto continua em background e a barra de rolagem cresce conforme carrega.
4. **Abas não visitadas podem ficar lazy** (carregadas ao clicar), com pré-carga em background quando a CPU estiver livre.
5. **Ignorado na leitura**: `calcChain.xml` (é só uma dica de ordem de cálculo; reconstruímos quando houver recálculo).
6. **Lazy ou só preservado**: pivot caches, data model do Power Pivot, slicers, timelines.
7. Parsers escritos à mão sobre `quick-xml` para os elementos quentes (`<row>`, `<c>`, `<v>`, `<f>`), sem serde genérico nem alocação por célula.

**Otimizações futuras.**

- **`.xlsb`** (formato binário do Excel): ordens de grandeza mais rápido de ler; bom alvo para "salvar como formato rápido".
- **`.csv`** e **`.ods`**.
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
| **F2** | **Visualizador completo** (v1): abas, estilos, merges, larguras, congelar painéis, busca paralela/SIMD, copiar, carga progressiva; inflate paralelo especulativo | Abre o arquivo de referência dentro das metas |
| **F3** | Edição + escrita com round-trip (salvar em paralelo) | Teste de round-trip verde no corpus |
| **F4** | Motor de cálculo: grafo de dependências por intervalos, recálculo incremental em níveis paralelos, fórmulas compartilhadas como kernels SIMD | Recalcula o arquivo de referência sem travar a UI |
| **F5+** | Gráficos, pivôs interativos, slicers, VBA, formato nativo (ADR-013), `duck-docs`, `duck-slides` | — |

### Status da F0 (18/11/2026)

Entregue em `platform/`: `duck-platform`, `duck-gfx`, `duck-text` e o exemplo `million_rows` (1.000.000 de linhas × 40 colunas, alturas variáveis, seleção, teclado, roda do mouse, scrollbars arrastáveis, clipboard). `duck-ui` (widgets) fica para quando houver o primeiro widget de verdade (F2).

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
- **Fontes carregam em paralelo** com a janela e a GPU (o `TextSystem` é `Send`); a espera por elas sumiu da abertura.
- **Caminho rápido para números** no `duck-text`: dos ~3,6 milhões de textos desenhados no benchmark, ~1,6 milhão foram números montados por caractere, sem shaping, e só ~2 mil strings precisaram de shaping completo.
- **Número que não cabe na célula vira `###`**, como no Excel: cortado, ele mostraria um valor errado.
- O `duck-gfx` também renderiza fora da tela (`render_to_rgba`, usado por `million_rows --screenshot`), base para testes visuais automáticos.

Pendências da F0:

- **Linux com GPU de verdade.** No Linux só foi testado o WSLg, com renderização por software. Falta um Linux nativo (ou WSL com driver de GPU) para medir o caminho acelerado.
- **Teclado e mouse no Linux** não foram testados por mim (só a renderização). Teste manual: `cargo run --release -p million_rows` no WSL, com setas, Ctrl+G, roda do mouse e arrastar as scrollbars.
- **Memória base de ~200 MB** com a janela aberta (boa parte é o driver da GPU). Medir e separar na F1, junto com o `duck-bench`.
- O CI (`.github/workflows/ci.yml`) está escrito, mas ainda não rodou: o repositório git é só local, sem remoto no GitHub.

---

## Questões em aberto

- **Licença** do projeto (MIT/Apache-2.0, GPL, outra).
- **Nome próprio** para a biblioteca de `platform/`, já que ela vai ser usada fora da suíte (por enquanto os crates usam o prefixo `duck-`).
- **Formato de arquivo nativo**: ideia registrada no ADR-013, com as perguntas que precisam de resposta antes de decidir.
- **Corpus de testes**: reunir planilhas públicas grandes e variadas além do gerador sintético.
- **Motor de cálculo**: avaliar compatibilidade numérica com o Excel (datas, precisão, funções financeiras) antes da F4.
