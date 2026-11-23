# Roadmap do duck-office

Onde o projeto está, o que já foi feito e o que falta. Os detalhes de cada decisão e as medições completas ficam em [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

**Fase atual: F5** (gráficos e segmentações de dados, que aparecem em quase toda planilha real). O motor de cálculo (F4) está pronto: 100% das fórmulas das planilhas de teste iguais ao Excel, recálculo do arquivo de referência em 0,6 s. Gráficos e segmentações já aparecem na tela; falta a interação (clicar na segmentação, tabelas dinâmicas). Falta o teste manual de teclado e mouse das fases anteriores.

| Fase | Entrega | Status |
|---|---|---|
| **F0** | Camada nativa (`platform/`) e grade de 1 milhão de linhas | ✅ Concluída |
| **F1** | Leitura paralela de `.xlsx`/`.xlsm` e benchmark | ✅ Concluída |
| **F2** | Visualizador `duck-sheets` | ✅ Concluída |
| **F3** | Edição e salvar com round-trip | ✅ Concluída (teste manual pendente) |
| **F3.5** | Casca do app: faixa de opções, menu Arquivo, clique direito, novo arquivo, "Abrir com"; filtro e classificação | ✅ Concluída (teste manual pendente) |
| **F4** | Motor de cálculo | ✅ Concluída (matrizes dinâmicas pendentes) |
| **F5+** | Gráficos, pivôs, slicers, VBA, formato nativo, docs e slides | 🔶 Em andamento (gráficos e segmentações na tela) |

---

## ✅ F0 · Camada nativa e grade de 1 milhão de linhas

- `duck-platform`: janela, teclado, mouse, DPI e clipboard (Windows, Linux e macOS).
- `duck-gfx`: renderer 2D por instâncias sobre wgpu, 1 draw call por frame, DX12 no Windows, Vulkan no Linux e Metal no macOS, renderização fora da tela para screenshots.
- `duck-text`: shaping, atlas de glifos, cache de texto e caminho rápido para números.
- `million_rows`: grade de 1 a 10 milhões de linhas.
- Regra no CI: `platform/` não depende de nada da suíte.

Resultado: ~0,8 ms de CPU por frame (1.150+ fps sem vsync) no Windows, e o mesmo custo com 10 milhões de linhas.

## ✅ F1 · Leitura paralela de planilhas

- `duck-core`: modelo colunar em blocos (11 bytes por célula na montagem, compactados no fim), strings contíguas, estilos.
- `duck-par`: pool de threads, limite de memória em trânsito, cortes por SIMD.
- `duck-deflate`: descompactação deflate **paralela e especulativa** (decodificador próprio).
- `duck-xlsx`: todas as abas ao mesmo tempo, abas grandes em pedaços paralelos, fórmulas (compartilhadas e deduplicadas), larguras, alturas, mesclagens e painéis.
- `duck-calc`: texto das fórmulas, modelos relativos.
- `duck-bench`: `open`, `sweep`, `verify` (contra o calamine), `gen`, `inflate`, `parse`.

Resultado no arquivo de referência (185 MB, 1,9 GB de XML, 24 milhões de células, 10 milhões de fórmulas): **370–420 ms** para ler tudo com 16 threads (era 450–650 ms), pico de ~355 MB, **0 divergências** contra o calamine.

Escalabilidade medida em 22/11/2026 (Ryzen 9 5950X, 16 núcleos físicos, melhor de 5 rodadas): 1 thread 3,58 s; 8 threads 0,58 s (6,2×); **16 threads 0,338 s (10,6×)**; 32 threads lógicas 0,343 s (os mesmos 16 núcleos). As duas metas estão atingidas (< 0,4 s e ≥ 10× com 16 núcleos). O que sobra entre 10,6× e o ideal: a frequência cai com todos os núcleos ocupados e a descompactação especulativa da aba de 1,6 GB gasta uns 25% a mais de CPU que a sequencial.

## ✅ F2 · Visualizador `duck-sheets`

Feito:

- **Abrir**: linha de comando, diálogo nativo (Ctrl+O), arrastar o arquivo para a janela; `.xlsx`, `.xlsm` e `.csv`.
- **Carga progressiva**: prévia da aba ativa → primeira tela → aba inteira → outras abas, cada uma quando fica pronta.
- **Aparência do Excel**:
  - formatos de número em pt-BR: moeda, contábil, %, datas e horas, científico, frações, cores e condições;
  - fontes (família, tamanho, negrito, itálico, sublinhado, cor), preenchimentos, cores do tema com tonalidade;
  - bordas (fina, média, grossa, dupla, pontilhada, tracejada), alinhamentos, recuo, texto quebrado;
  - larguras e alturas reais, linhas e colunas ocultas, texto que transborda para as vizinhas, `###`;
  - texto girado e empilhado, texto rico (várias fontes na célula), tachado, estilo de linha inteira;
  - formatação condicional: regras de valor, fórmula, texto e data, escalas de cor, barras de dados, ícones, N maiores, acima da média, repetidos.
- **Estrutura**: abas (ocultas ficam de fora, com a cor da guia), mesclagens, painéis congelados, barra de fórmulas, caixa de nome ("ir para").
- **Interação**: seleção com mouse, Shift e cabeçalhos; Ctrl+setas; copiar em TSV (Ctrl+C); soma, média e contagem da seleção; busca paralela sem acentos (Ctrl+F); zoom; redimensionar colunas e linhas arrastando o cabeçalho (duplo clique ajusta); hiperlinks com Ctrl+clique.
- **Plataforma**: crate novo `duck-ui` (Painter, campo de texto, abas), texto com estilos e quebra de linha, bordas tracejadas no shader.
- **Medição**: `--bench`, `--screenshot`, `duck-bench preview` e uma planilha de vitrine sem dados reais (`duck-bench gen-showcase`) no CI.

Resultado no Windows (AMD RX 9070 XT, DX12): **primeira tela com dados em 190–199 ms** (era 212–245), todas as 20 abas em 0,59–0,67 s, ~860–1.090 fps sem vsync (p99 do frame até 2,5 ms), **memória de 422–437 MB** com tudo carregado (era 657–663 MB) e **pico de 517–527 MB** durante a leitura (era 954–979 MB).

No Linux (WSLg, renderização por software): primeira tela em 125–182 ms, todas as abas em 0,75–0,95 s, frame montado em até 1,4 ms (p99), busca em 593 mil células em 33 ms, memória de 401–412 MB e pico de 467–489 MB (era 652–658 MB).

Para não atrasar a primeira tela, a GPU começa a abrir numa thread desde o início do processo (no DX12; a janela só cria a superfície), a leitura da planilha corre junto, e só 12 fontes essenciais são carregadas no início. As outras ~340 fontes do sistema são lidas numa thread depois da primeira tela; com o cache de disco frio, isso levava até 0,6 s.

Falta na F2:

- [x] Medir no Windows com a GPU (21/11/2026).
- [x] Memória abaixo de 600 MB: 423–437 MB no Windows, com os blocos de células compactados (dados: 396 → 193 MB).
- [x] Pico de memória durante a leitura: 517–527 MB no Windows e 475–489 MB no Linux (era ~960 e ~655 MB), com a leitura completa ~15% mais rápida.
- [x] Abrir a GPU antes da janela existir: primeira tela de 212–245 para 190–199 ms no Windows (no Linux a escolha do adaptador depende da janela e fica como estava).
- [ ] Teste manual de teclado e mouse na janela (redimensionar e Ctrl+clique em link ainda não foram usados à mão).
- [x] Texto girado (`textRotation`) e empilhado.
- [x] Formatação condicional (fora: regras `x14` do Excel 2010 e fórmulas com outras abas, intervalos ou funções fora do avaliador).
- [x] Estilo de linha inteira (`<row s>`).
- [x] Várias fontes na mesma célula (rich text) nas strings compartilhadas.
- [x] Redimensionar colunas e linhas com o mouse (duplo clique ajusta a coluna ao texto).
- [x] Hiperlinks (Ctrl+clique; externos só `http`, `https` e `mailto`).
- [x] Abrir `.csv` (separador detectado, vírgula decimal com `;`).

## ✅ F3 · Edição e salvar

- [x] **Salvar por remendo** (ADR-014): partes intactas copiadas com os bytes comprimidos originais; numa aba editada, só as linhas e células mudadas são reescritas, o resto é copiado byte a byte; compressão em paralelo.
- [x] **Conferência antes de substituir**: o arquivo é gravado num temporário, as abas regravadas são relidas e comparadas célula a célula com o modelo, e só então o original é trocado.
- [x] Camada de comandos com desfazer/refazer (`duck_core::Command`), inclusive passos que mexem em várias abas.
- [x] Editar células: na célula ou na barra de fórmulas; o que se digita é lido como no Excel em português (números, %, moeda, datas, horas, lógicos, fórmulas).
- [x] Apagar, copiar, recortar e colar (de outros programas também); colar dentro do app leva fórmulas (deslocadas) e estilos.
- [x] Formatação básica: negrito, itálico, sublinhado, tachado, cor do texto e do fundo, alinhamento, quebra de texto, formatos de número, bordas; larguras e alturas.
- [x] Inserir e apagar linhas e colunas, com as fórmulas de todas as abas, os nomes definidos, mesclagens, formatação condicional, validação e links ajustados.
- [x] `.csv` salvo com o mesmo separador e codificação.
- [x] `duck-bench roundtrip` (cópia intacta, reescrita total, edições, estrutura) no CI e no arquivo de referência: **0 divergências**.
- [ ] Teste manual de teclado e mouse na janela.

Resultado no arquivo de referência (Windows, 16 threads):

- salvar sem mudanças: **176–210 ms** (701 partes copiadas, idênticas byte a byte);
- 500 edições na aba de 1,6 GB de XML: gravação em **1,5 s** e conferência em 0,9 s; o arquivo cai de 185 para 129 MB (o `calcChain.xml` sai e o deflate é o nível 6);
- editar e salvar pelo app, abrindo o arquivo: **menos de 1 s** no total;
- inserir ou apagar linhas/colunas na aba de 593 mil linhas: **~1 s** cada; regravar a aba (1,6 GB de XML gerados do modelo em paralelo): 1,9 s + 0,9 s de conferência.

Limites desta versão:

- fórmulas novas só têm valor se o avaliador simples entender (sem intervalos como `SUM`); as que dependem da célula editada não mudam. O motor de cálculo vem na F4, e o arquivo salvo pede ao Excel para recalcular ao abrir;
- inserir/apagar linhas não move gráficos, imagens, tabelas e tabelas dinâmicas (o app avisa antes); na aba regravada, `cm`/`vm` das células e níveis de agrupamento das linhas não voltam;
- texto rico de células reescritas vira texto simples; quebra de linha dentro da célula (Alt+Enter) ainda não existe;
- os nomes das funções são os do arquivo (inglês, separador `,`).

## ✅ F3.5 · Casca do app

Sem isso o editor só é usável por atalhos. O objetivo é que tudo o que a F3 já faz esteja à vista, como num editor de planilhas comum, e que o duck-sheets se integre ao sistema.

- [x] **Faixa de opções** com abas (Página Inicial, Inserir, Exibir) e grupos, no lugar da barra de ferramentas atual; atalhos rápidos (salvar, desfazer, refazer) no canto.
- [x] **Página Inicial**:
  - Área de transferência: colar, recortar, copiar;
  - Fonte: família e tamanho (listas), aumentar/diminuir, negrito, itálico, sublinhado, tachado, bordas, cor do fundo, cor do texto;
  - Alinhamento: cima/meio/baixo, esquerda/centro/direita, recuo, quebrar texto, mesclar e centralizar;
  - Número: lista de formatos (Geral, Número, Moeda, Contábil, Data, Hora, Porcentagem, Texto), moeda, %, milhar, mais/menos casas decimais;
  - Células: inserir e excluir (linhas, colunas), formatar (largura da coluna, altura da linha, ajustar, ocultar e reexibir);
  - Edição: limpar (tudo, formatos, conteúdo), localizar, ir para.
- [x] **Inserir**: linhas, colunas, mesclagem. **Exibir**: zoom, linhas de grade, cabeçalhos, congelar painéis (salvo no arquivo).
- [x] **Menu Arquivo**: novo, abrir, arquivos recentes, salvar, salvar como, "Abrir com", sair.
- [x] **Novo arquivo**: planilha em branco que pede onde salvar na primeira vez.
- [x] **Menu de clique direito** na grade e nos cabeçalhos (recortar, copiar, colar, inserir, excluir, limpar, mesclar, largura/altura, ocultar).
- [x] **Gravador**: mesclagens, linhas/colunas ocultas, formatos de número novos e painéis congelados também saem no arquivo salvo.
- [x] **Integração com o sistema**: registrar o duck-sheets no "Abrir com" de `.xlsx`/`.xlsm`/`.csv` (Windows e Linux), feito só quando o usuário pedir pelo menu.
- [ ] Teste manual de teclado e mouse na janela.

Segunda parte (aba **Dados**):

- [x] **Filtro automático**: as setas nos cabeçalhos (`autoFilter` da aba e das tabelas do Excel), lista de valores com busca e marcar/desmarcar (calculada em paralelo, só com as linhas que passam nos filtros das outras colunas); o resultado esconde as linhas como no Excel e é salvo no arquivo. Critérios que o Excel grava e o duck-sheets ainda não edita (personalizado, 10 maiores, por cor, por data) ficam guardados como estão.
- [x] **Classificar** pela coluna da célula ativa (crescente/decrescente), dentro do filtro, da tabela ou da região de dados, em paralelo e com desfazer. Classificar por várias colunas ao mesmo tempo fica para depois.
- [x] Desenho das **tabelas do Excel** (cabeçalho, faixas alternadas) a partir do estilo da tabela e das cores do tema; editar o cabeçalho atualiza o nome da coluna, e inserir/apagar linhas e colunas ajusta a tabela (o Excel exige os dois em dia).

Critério de pronto: dá para abrir, editar, formatar, filtrar e salvar uma planilha só com o mouse.

## ✅ F4 · Motor de cálculo

- [x] **Leitor de fórmulas** (`duck-calc/src/parse.rs`): texto → fórmula compilada que não depende da célula (cada referência é absoluta ou um deslocamento). Um modelo copiado para um milhão de linhas é compilado uma vez. Abas, referências de tabela (`Tabela[[#This Row],[Coluna]]`, `[@Coluna]`), nomes definidos e arquivos externos (`[1]Aba!A:F`, com os valores que o Excel guardou) são resolvidos na compilação.
- [x] **Funções** (150 calculadas): lógicas, informação, matemáticas e estatísticas, critérios (SOMASES, CONT.SES, MÉDIASES...), texto (TEXTO com códigos em português), datas (FIMMÊS, NÚMSEMANA, DATADIF...) e busca (PROCV, PROCX, CORRESP, ÍNDICE, DESLOC, INDIRETO). As que ainda não são calculadas (INFODADOSTABELADINÂMICA, FILTRO, ÚNICO...) mantêm o valor guardado.
- [x] **Buscas com índice**: PROCV, CORRESP, PROCX e os critérios de igualdade montam um índice por coluna uma vez e o compartilham entre as threads.
- [x] **Grafo de dependências por intervalos**: células seguidas com a mesma fórmula viram um bloco; referências viram retângulos; níveis calculados em paralelo; blocos que dependem de si mesmos (saldo acumulado) de cima para baixo.
- [x] **Recálculo incremental em segundo plano**: ao abrir (voláteis, como o Excel) e a cada edição, só os dependentes e os voláteis. A janela continua respondendo; uma edição nova cancela o cálculo em andamento. F9 recalcula os voláteis, Shift+F9 tudo.
- [x] **Fórmulas em português** na barra e ao digitar (`=SOMA(A1;1,5)`); o arquivo guarda como o Excel (`SUM(A1,1.5)`).
- [x] **Conferência** (`duck-bench calc`): cada fórmula do arquivo é calculada com os valores guardados e comparada com o resultado do Excel.
- [ ] Fórmulas de matriz dinâmica (FILTRO, ÚNICO, CLASSIFICAR, SEQUÊNCIA) e LET/LAMBDA.
- [ ] Núcleos SIMD para blocos de fórmulas simples (hoje cada célula é avaliada pela árvore).

Resultado: nas duas planilhas reais de teste, **100% das fórmulas iguais ao Excel** (10,1 milhões e 140 mil). O arquivo de referência inteiro é recalculado em **0,6 s** com 32 threads (grafo montado em 30 ms); só os voláteis (5 milhões de células com HOJE) em 0,2 s.

Critério de pronto: recalcular o arquivo de referência sem travar a janela. ✅

## 🔶 F5+ · O resto da suíte

- [x] **Gráficos na tela**: barras (verticais, horizontais, agrupadas, empilhadas, 100%), linhas (também empilhadas e tracejadas), área, pizza e rosca, combinações com eixo secundário, rótulos de dados (com caixa colorida), legenda, título, linhas de grade, escala fixa do eixo e as cores do tema com os ajustes do arquivo (`lumMod`, padrões, gradientes). Os valores vêm das células: o gráfico muda quando a fórmula de origem é recalculada.
- [x] **Segmentações de dados na tela**: de tabela dinâmica (itens do cache, com o que está selecionado) e de tabela do Excel (valores da coluna, marcados pelo filtro da tabela), com o estilo do arquivo (`x14:slicerStyles`) ou os estilos prontos.
- [x] Formas com texto e grupos de formas (as posições dos filhos são compostas até a âncora).
- [ ] Imagens (hoje lidas, não desenhadas), edição de gráficos, gráficos de dispersão completos.
- [ ] Clicar na segmentação filtra a tabela (as de tabela do Excel já sabem qual coluna); as de tabela dinâmica dependem do motor de tabelas dinâmicas.
- [ ] Tabelas dinâmicas interativas e linhas do tempo (timelines).
- [ ] Inserir/apagar linhas e colunas move os objetos (hoje o app avisa e eles ficam onde estavam).
- [ ] VBA (hoje só preservado).
- [ ] Formato de arquivo nativo (ADR-013, em discussão).
- [x] macOS: compila, roda e passa no CI (Metal); o pacote `.app` assinado ainda não existe.
- [ ] `duck-docs` e `duck-slides` sobre a mesma camada `platform/`.

## Questões em aberto

- Nome próprio para a biblioteca de `platform/`.
- Corpus de testes com planilhas públicas grandes e variadas.
