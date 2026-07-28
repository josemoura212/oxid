# ROADMAP — Encurtador de URL em Rust

Regras de progressão: uma etapa por vez, na ordem. Uma etapa só é concluída quando
todos os critérios de aceite passam. Ao concluir, marcar os checkboxes e registrar
aprendizados/decisões em `docs/DECISOES.md`.

Legenda: 🎯 = critério de aceite | 🦀 = conceito de Rust a dominar nesta etapa

---

## Etapa 1 — Fundação async ✅

- [x] `cargo new url-shortener` + dependências: tokio (full), axum
- [x] Rota `GET /health` retornando 200 com JSON
- [x] Struct `AppState` (vazio por ora) circulando via `State<Arc<AppState>>`
- [x] `tracing-subscriber` inicializado no main com `TraceLayer` do tower-http

🎯 `curl localhost:3000/health` responde e o log estruturado da request aparece.
🦀 Runtime Tokio, handlers async, extractors, por que não bloquear o runtime.

## Etapa 2 — Base62 + bijeção ofuscadora ✅

- [x] `codec/base62.rs`: `encode(u64) -> String` e `decode(&str) -> Option<u64>`
- [x] `codec/obfuscate.rs`: bijeção sobre u64 (multiplicação modular por primo
      com inverso pré-calculado, OU rede de Feistel de 3-4 rounds)
- [x] Testes: roundtrip encode/decode, roundtrip obfuscate/deobfuscate,
      rejeição de caracteres inválidos, casos extremos (0, u64::MAX no domínio)

🎯 `cargo test` verde; dois IDs consecutivos geram códigos visualmente não relacionados.
🦀 Módulos, `Option`/`Result`, `#[cfg(test)]`, aritmética com `wrapping_*`/`checked_*`.

## Etapa 3 — Persistência com sqlx ✅

- [x] Postgres local via docker-compose (`infra/docker-compose.yml`)
- [x] Migration: tabela `urls` (id bigserial PK, url_hash unique gerado, long_url,
      created_at). **Sem `short_code`**: o código é função pura do id, guardá-lo
      seria dado derivado que pode divergir — ver `docs/DECISOES.md`.
- [x] `PgPool` no `AppState`, criado no main com `max_connections` vindo de config
- [x] `repo.rs`: inserir com `ON CONFLICT DO NOTHING RETURNING id` + SELECT fallback
      (idempotência no banco, não na aplicação)
- [x] `sqlx::query_scalar!` compilando contra o schema real (+ `.sqlx/` para build offline)

🎯 Inserir a mesma URL duas vezes retorna o MESMO id, provado por teste de integração.
🦀 async com banco, macros do sqlx, `DATABASE_URL` em `.env`, `Result` + `?`.

## Etapa 4 — Rotas de escrita e leitura ✅

- [x] `error.rs`: enum `AppError` (NotFound, InvalidUrl, InvalidBody, Database, Internal)
      com `impl IntoResponse`. Corpo segue **RFC 9457** (`application/problem+json`).
- [x] `POST /v1/shorten`: `Json<ShortenRequest>`, validação http/https (crate `url`),
      fluxo id → obfuscate → base62 → responder
- [x] `GET /{code}` — **não** `/v1/urls/{code}`: o prefixo gastaria 9 chars numa URL
      cujo objetivo é ser curta. 301 montado à mão (`Redirect::permanent` emite 308).
- [x] 404 limpo para código inexistente **e para código malformado** (separar os dois
      vazaria o formato do shortcode), 400 para URL inválida

🎯 Fluxo completo via curl: encurtar → seguir redirect → chegar na URL original.
🦀 `Deserialize`/`Serialize` com serde, `?` propagando para `AppError`, `IntoResponse`.

## Etapa 5 — Cache Redis (cache-aside + cache negativo) ✅

- [x] Redis no docker-compose com `maxmemory` definido e `allkeys-lru`
- [x] Cliente Redis no `AppState` (crate `redis` 1.x, não `fred` — ver `docs/DECISOES.md`)
- [x] `cache.rs`: no GET, tentar cache → miss → banco → gravar no cache SEMPRE (sem TTL)
- [x] Popular o cache também na escrita (o dado acabou de nascer quente)
- [x] Cache negativo: código inexistente grava sentinela com **SET NX + TTL curto**
- [x] Contadores de hit/miss (`tracing` com campo `cache=hit|miss|hit_negative`)
- [x] **Extra:** rate limit por IP no `POST /v1/shorten` (`tower_governor`).
      O redirect fica sem limite de propósito — é o caminho que o cache absorve
      e que as Etapas 9-10 empurram a 11k req/s.

🎯 Segunda leitura do mesmo código não toca o Postgres (provar por log/métrica).
🎯 Teste da corrida: escrita concorrente positiva nunca é sobrescrita por negativa.
🦀 Traits de cliente async, serialização para o cache, TTL condicional.

## Etapa 5.1 — "Minhas URLs" sem login (só front) ✅

- [x] `localStorage` guarda os códigos criados neste browser
- [x] Listar com destino e link curto; ação de **remover da lista**
- [x] Deixar explícito na UI: salvo só neste navegador, e remover não desativa o link
- [x] **Extra:** redesign do front — medidor de compressão, paleta ferro/óxido,
      JetBrains Mono self-hosted subsetada (5 KB por peso), botão de copiar

🎯 Fechar o browser e voltar mantém a lista; limpar dados do site zera.
🦀 Persistência no browser via `web-sys`, estado derivado com signals do Leptos.

**Por que esta etapa existe:** encurtar e perder o código é o único jeito de o produto
falhar sem dar erro. A necessidade apareceu com o site no ar — sem lista, quem fecha a
aba perde o link, e não há nenhuma forma de recuperá-lo (a busca é por código, nunca
por URL longa).

**Por que não tem back:** o código é imutável, e é isso que sustenta o cache sem TTL
(Etapa 5) e o 301. Exclusão real exigiria invalidar o Redis e trocar 301 por 302 —
e nem assim funcionaria, porque um 301 já cacheado no browser redireciona para sempre.
Além disso a idempotência é global: o mesmo código pode ter sido criado por várias
pessoas, então apagar seria apagar o link de outro. Ver `docs/DECISOES.md`.

A Etapa 12 troca o 301 por 302 **só nas URLs com dono** — o que não contradiz o
parágrafo acima: continua não havendo exclusão, e o 302 existe para contar clique, não
para permitir desativar link.

Precursor das Etapas 11 e 12: a lista por browser vira lista por conta quando houver
login. Nada aqui vira dívida — a lista local continua valendo para quem não se cadastrar.

## Etapa 5.2 — Acertos do Lighthouse (só o que é barato)

Linha de base de 2026-07-26 e a análise completa em `docs/PERFORMANCE-WEB.md`:
desempenho 99 (mobile) / 100 (desktop), acessibilidade **91**, TBT 0, CLS 0.
Performance já está no teto — o que sobra é acessibilidade e segurança.

- [x] **Contraste**: resolvido no redesign da 5.1. Os tokens viraram três papéis
      (`--accent-face`, `--accent-ink`, `--accent-text`) em vez de um `--accent`
      servindo fundo claro e escuro, que era a raiz do problema
- [x] `<label>` oculto no input (havia só `placeholder`, que some ao digitar) e
      `aria-live="polite"` no resultado, senão o leitor de tela não anuncia o link gerado
- [x] `100svh` no lugar de `100vh`; alvo de toque do botão em 44 px; `flex-wrap` no
      formulário abaixo de ~420 px; `overflow-wrap: anywhere` no lugar de `break-all`
- [x] CSS inline via `data-trunk rel="inline"` — elimina os 150 ms de bloqueio de
      renderização, ao custo de o CSS viajar dentro do HTML `no-cache`
- [x] **Extra:** `preload` das duas fontes. Como o `<body>` vai vazio, nada pedia por elas
      até o wasm montar a UI — exatamente o instante em que a página tem o que mostrar
- [x] Headers de segurança no Traefik: HSTS, `frame-ancestors`, COOP, nosniff,
      `Referrer-Policy` — **aplicados em 2026-07-26**, verificados pela Cloudflare e direto
      na origem

🎯 ✅ **Acessibilidade 91 → 100** confirmado em 2026-07-26, com desempenho 98, práticas
   recomendadas 100, SEO 100 e navegação agêntica 2/2.

**Sobre "chegar a 100 em desempenho":** 98 e 99 são a mesma medição com ruído — o índice
oscila entre execuções do mesmo build. O que dava para atacar objetivamente era o bloqueio
de renderização, e ele saiu. O que sobra é o script que a Cloudflare injeta e o tamanho do
wasm (reativar `wasm-opt` quando o binaryen arm64 funcionar).

**Fora desta etapa, de propósito:** CSP (o trunk emite o bootstrap do wasm como script
inline, então exige hash por build ou nonce — trabalho de verdade) e markup estático/SSR
para melhorar FCP. Os dois estão em `docs/PERFORMANCE-WEB.md` com o custo estimado.

**Não é problema nosso:** os avisos de cache de 5 KiB e parte dos "3 KiB de JS" vêm do
script que a Cloudflare injeta no `<body>` (`max-age=300`, verificado em produção).
Some desligando *Bot Fight Mode* — decisão de segurança, não de performance.

## Etapa 5.3 — Idioma pelo navegador ✅

- [x] Detectar por `navigator.language` (via `web-sys`), **com pt-BR como padrão**
- [x] **Seletor visível**, e não só detecção — quem usa o sistema em inglês e lê português
      fica preso sem ele. A escolha explícita grava em `localStorage`
      (`oxid.locale.v1`, ao lado da lista) e vence o navegador
- [x] `document.documentElement.lang` corrigido no mount — é esse atributo que o leitor de
      tela usa para escolher a pronúncia
- [x] `<title>` e `<meta description>` acompanham o idioma escolhido
- [x] Todas as strings num catálogo só, nenhuma literal solta no `view!`

🎯 ✅ Abrir com o navegador em pt-BR mostra a interface em português; trocar no seletor
   sobrevive ao reload.
🦀 `navigator.language` via web-sys, catálogo `&'static`, sinal de locale.

**Decisão — `match`, sem crate.** `leptos_i18n` traz Fluent, plural e interpolação; com dois
idiomas e 19 strings, um `enum Locale` com catálogo `&'static Strings` resolve sem
dependência nenhuma. A conta vira quando aparecer plural de verdade ou formatação de data.

**Decisão — o `index.html` declara `lang="pt-BR"`.** O documento estático é o que o crawler
lê e o que pinta antes do wasm montar. Declarar `en` e trocar depois significaria anunciar o
idioma errado ao leitor de tela por todo o tempo de carga do bundle.

**Ficou como está — o erro da API continua em inglês.** O `detail` da RFC 9457 é gerado pelo
servidor. Traduzir no front, casando por `title`, duplicaria o catálogo e serviria só a este
cliente; o certo é negociar `Accept-Language` na API, e isso é i18n no back — trabalho
próprio, não um apêndice desta etapa.

**O ponto não óbvio — o erro vem do servidor em inglês.** O `detail` da RFC 9457 é gerado
pela API. Duas saídas, e elas não são equivalentes:

- **Traduzir no front, mapeando por `title`/`type`.** O contrato já promete que esses campos
  são estáveis justamente para o cliente casar em cima deles. Não toca no back, mas duplica
  catálogo e só serve a este front.
- **Negociar no back por `Accept-Language`.** Mais correto: quem chama a API por `curl` ou
  script recebe o erro no idioma pedido. É o único caminho se um dia houver outro cliente —
  e implica i18n no back também.

**Limite conhecido:** com CSR, o crawler recebe o HTML estático em inglês qualquer que seja o
leitor. `hreflang` e título traduzido só valem de verdade com SSR, que está fora de escopo.

## Etapa 6 — Configuração e dimensionamento ✅

- [x] Toda config fora do código: `base.yaml` → `<ambiente>.yaml` → `APP_*`
      (YAML em vez de `.env`; ver `docs/DECISOES.md`, Etapa 3, decisão 6)
- [x] `.env.example` documentado
- [x] Pool do Postgres pequeno por padrão — `max_connections: 8`
- [x] Timeouts de acquire do pool (3 s) e de conexão do Redis (2 s)
- [x] **`statement_timeout` (3 s)** via `PgConnectOptions::options`, aplicado por conexão em
      vez de depender de config do servidor — o mesmo banco pode servir uma migration ou uma
      sessão manual que legitimamente demore mais
- [ ] Reavaliar `idle_timeout` e `max_lifetime` do pool — só faz sentido com número na mão,
      depois da Etapa 9

🎯 ✅ App sobe em ambiente limpo só com o YAML preenchido; pool visível nas métricas.
🦀 Structs de config com serde, `Duration`, fail-fast no bootstrap.

**Por que o `statement_timeout` importa mais aqui do que parece:** com pool grande, uma query
lenta degrada; com pool de 8, ela **esgota**. É o tipo de falha que só aparece sob carga —
ou seja, na Etapa 9, quando o custo de descobrir é bem maior.

## Etapa 7 — Observabilidade

- [x] `metrics` + `metrics-exporter-prometheus`, endpoint `/metrics` **em porta própria**
- [x] Histograma de latência por rota, contador `cache_lookups_total` por desfecho,
      gauges do pool
- [x] Prometheus + Grafana em `infra/docker-compose.observability.yml`
- [x] Dashboard provisionado por arquivo: p50/p95/p99 por rota, req/s por status, hit rate,
      lookups por desfecho, conexões do pool
- [ ] Exporters de Postgres, Redis e Nginx — entram junto com a Etapa 8, quando o Nginx
      existir
- [ ] Prometheus no cluster raspando os pods (hoje só o compose local)

🎯 Dá para responder "onde está o gargalo?" olhando um único dashboard.
🦀 Macros de métricas, custo de instrumentar, cardinalidade de labels.

**`/metrics` não é rota do router público.** O Traefik encaminha para a API tudo que não é
o front, então uma rota `/metrics` estaria legível na internet — entregando volume de
requisições, distribuição de latência e comportamento do cache a quem pedisse. Vai num
listener separado (9090), declarado no Deployment e **ausente do Service**.

**O label de rota vem do `MatchedPath`, não do path.** Em `/{code}` os dois diferem por
construção: o path real é uma string diferente a cada request, então rotular com ele criaria
uma série temporal por shortcode e derrubaria o Prometheus muito antes do serviço. É a única
linha do middleware que não pode estar errada.

**Buckets escolhidos, não os padrão.** O conjunto default se espalha por uma faixa que este
serviço nunca usa. A meta da Etapa 10 é p95 < 50 ms e um hit de cache responde em
milissegundos de um dígito — a resolução tem que estar embaixo de 100 ms, que é onde as
respostas caem.

**Gauges do pool amostrados no scrape**, não por task com timer: lidos na hora do scrape
nunca estão mais velhos que o próprio scrape, e não há um segundo relógio para raciocinar.
`total - idle` é o número que importa; grudado em `max_connections` significa fila no pool,
não no banco.

## Etapa 8 — Topologia completa

- [ ] 2 instâncias da app atrás de Nginx (docker-compose primeiro; VMs depois, se houver homelab)
- [ ] Nginx: upstream com keepalive, `keepalive_requests` alto, worker_processes = cores
- [ ] Sysctls documentados em `infra/`: `tcp_tw_reuse`, faixa de portas efêmeras, `somaxconn`
- [ ] Build de produção: `cargo build --release`, binário em imagem slim (multi-stage)

🎯 Tráfego balanceado 50/50 nas duas instâncias, verificado nas métricas.

## Etapa 9 — Teste de carga com k6

- [ ] `infra/k6/load.js`: executor `ramping-arrival-rate`, rampa de 30s,
      proporção 1:10 escrita/leitura, pool de URLs pré-criadas para as leituras
- [ ] Thresholds no script: p95 alvo e taxa de erro 0
- [ ] Checklist de validade do teste: gerador com folga de CPU, `dropped_iterations = 0`,
      app em `--release`
- [ ] Rodar em escala 0.5 primeiro (≈ 5.800 leituras/s + 580 escritas/s)

🎯 Relatório da escala 0.5 com percentis limpos e zero erros — ou gargalos
   identificados com telemetria dos dois lados (cliente E servidor).

## Etapa 10 — Ciclo de otimização até escala 1.0

- [ ] Método fixo: hipótese → medição → mudança (UMA por vez) → confirmação
- [ ] Registrar cada iteração em `docs/DECISOES.md` (o que media, o que mudou, resultado)
- [ ] Subir escala: 0.5 → 0.75 → 1.0 (≈ 11.574 leituras/s + 1.157 escritas/s)
- [ ] Suspeitos prováveis, nesta ordem: Nginx/kernel (portas, sockets), pool do Postgres,
      serialização no hot path, límites de file descriptors

🎯 Escala 1.0 sustentada com zero erros e p95 de leitura < 50ms.

---

## Etapa 11 — Contas e sessão

Depois da Etapa 10, de propósito: autenticação não muda o perfil de carga do sistema,
e as Etapas 9-10 medem melhor um sistema sem sessão no caminho.

- [ ] Migration `users` (id, email `citext` unique, `password_hash`, created_at)
- [ ] Migration `url_owners` (user_id, url_id, created_at) — PK composta, índice
      `(user_id, created_at DESC)` para a listagem
- [ ] Hash argon2id; verificação sempre em tempo constante, inclusive para e-mail
      inexistente (hash falso), senão o tempo de resposta vira oráculo de cadastro
- [ ] Sessão no Redis, cookie `HttpOnly` + `Secure` + `SameSite=Lax`, id de 128 bits
- [ ] `POST /v1/signup`, `POST /v1/login`, `POST /v1/logout`, `GET /v1/me`
- [ ] `GET /v1/urls` — lista do dono, paginada por keyset (não OFFSET)
- [ ] `POST /v1/shorten` associa ao dono quando há sessão; sem sessão segue igual
- [ ] Rate limit no login, separado do de `shorten`

🎯 Duas contas encurtando a MESMA URL longa recebem o MESMO código, e cada uma
   a vê só na sua lista — a idempotência global sobrevive ao ownership.
🦀 Middleware/extractor de auth no Axum, `argon2`, cookies assinados, keyset pagination.

**Decisão (2026-07-26): ownership é N:N, a idempotência global fica intacta.**
`urls.url_hash` continua `UNIQUE` global. A alternativa — `UNIQUE (user_id, url_hash)`,
código próprio por usuário — daria métrica isolada por dono, mas multiplicaria linhas
num modelo que projeta 365 bilhões delas contando com o dedupe.

## Etapa 12 — Analytics de clique

- [ ] `302` quando o código tem dono, `301` quando não tem — o caminho anônimo
      continua cacheável pelo browser e é o que o k6 exercita
- [ ] Flag `owned` no valor cacheado; **monotônica** (`false → true`, nunca volta),
      com invalidação da chave no exato momento em que o código ganha o primeiro dono
- [ ] Evento por clique sai do hot path: `mpsc` → worker → insert em lote
- [ ] **Os dois destinos no código**, escolhidos por config: `analytics.backend =
      postgres | clickhouse | off`
- [ ] Captura: ts, url_id, país (`CF-IPCountry`), browser/OS/dispositivo do user-agent,
      host do referer, idioma, flag de bot
- [ ] Visitante único sem cookie: `hash(ip + user-agent + salt do dia)` — o salt diário
      é o que impede reidentificar alguém entre dias
- [ ] Dashboard: cliques totais e únicos, série temporal, top países, top referrers,
      dispositivos

🎯 Um clique aparece no dashboard sem que o p95 do redirect se mova.
🦀 Canal `mpsc` com backpressure, task de background, batch de escrita.

### Onde gravar o evento — as duas implementações, trocadas por config

Não escolher no papel: implementar as duas atrás do mesmo tipo e alternar por
configuração, do jeito que `Cache::disabled()` (Etapa 5) já faz. Assim os dois medem o
**mesmo tráfego** em vez de dois testes que não se comparam — que é o método fixo da
Etapa 10 aplicado a uma decisão de banco.

```rust
// Enum, não `Box<dyn Trait>`: são três variantes conhecidas em tempo de compilação,
// e `dyn` com `async fn` ainda exigiria `#[async_trait]` e uma alocação por chamada.
enum ClickSink {
    Disabled,
    Postgres(PgPool),
    ClickHouse(ClickHouseClient),
}

impl ClickSink {
    async fn record(&self, batch: &[ClickEvent]) -> Result<(), SinkError> { todo!() }
    async fn summary(&self, url_id: i64, range: DateRange) -> Result<Summary, SinkError> { todo!() }
}
```

**Onde essa abstração é barata e onde ela dói.** A escrita abstrai bem: `record` recebe
um lote e devolve `Result` — as duas implementações fazem literalmente isso. A leitura
**não** abstrai: as consultas do dashboard são dialetos diferentes (`date_trunc` +
`GROUP BY` contra `toStartOfDay` e funções de agregação do ClickHouse), então são dois
conjuntos de query devolvendo o mesmo `Summary`. É aí que mora o custo de manter as
duas, e é o que a assinatura acima deixa explícito ao separar `record` de `summary`.

**Contrapartida honesta:** duas implementações é o dobro de superfície para um projeto
que ainda não tem observabilidade (Etapa 7). O que paga é o interruptor permitir
responder "quanto o ClickHouse ganha aqui, de verdade?" com número em vez de opinião —
e desligar (`off`) durante as Etapas 9-10, para a analytics não contaminar a medição.

### Comparação para orientar o default

| | **A. Postgres particionado** | **B. ClickHouse** |
|---|---|---|
| Infra nova | nenhuma | mais um banco no nó de 2 vCPU |
| Escrita | `COPY`/insert em lote na tabela particionada por mês | `async_insert` ou lote de 10k+ |
| Agregação sobre milhões | índice ajuda até certo ponto; depois exige tabela de rollup | é o que ele faz de melhor |
| Retenção | `DROP PARTITION` | `TTL` na tabela |
| Custo de errar | baixo — dá para migrar depois | alto — tirar um banco do ar é pior que não colocar |
| Aprendizado | mais SQL e particionamento | um paradigma novo (colunar, merges, partes) |

**Default sugerido: `postgres`.** Enquanto o volume couber em "milhões por mês" e as
consultas forem as seis do dashboard, a tabela particionada resolve — e subir o
ClickHouse é opcional, não requisito para a etapa fechar. O `clickhouse` entra quando
aparecer consulta ad-hoc sobre o histórico inteiro ou ingestão que o Postgres não
sustente sem virar gargalo do redirect. Com o interruptor, essa virada é uma linha de
config e uma medição, não um rewrite.

O ADR de 2026-07-26 já dizia que a analytics de clique é o bom caso do ClickHouse; o
que ele não dizia é que "bom caso" e "vale o custo agora" são perguntas diferentes.

**Três coisas que esta etapa quebra, e a resposta de cada uma:**

1. **O 301 impede contar cliques** — o browser cacheia e o segundo clique nunca chega
   ao servidor. Por isso só a URL com dono vira 302.
2. **Um dono faz o código inteiro virar 302**, inclusive para quem chegou pelo link
   criado anonimamente. Consequência direta do ownership N:N; aceita conscientemente.
3. **O cache sem TTL supunha imutabilidade total** (Etapa 5). Com `owned` no valor
   cacheado isso deixa de valer. A imutabilidade é recuperada tornando o flag
   monotônico: só há um instante de invalidação por código, na primeira reivindicação.
   Remover a URL da lista **não** devolve o 301 — e não poderia, porque um 301 já
   cacheado no browser é irreversível.

**Restrição de infra que vale para as duas opções:** o nó é **2 vCPU / 12 GB, arm64**,
e já roda Postgres, Redis, 2 réplicas da API e o nginx. É esse orçamento — e não a
qualidade do banco — que faz a opção A começar na frente.

## Etapa 13 — Extensão de navegador

Um clique no ícone encurta a página aberta e põe o link curto na área de transferência.
É onde o produto encosta no uso real: hoje encurtar exige sair da página, abrir o oxid,
copiar a URL e colar. A extensão apaga esses quatro passos.

Depois da Etapa 11 de propósito — sem conta a extensão só encurta; com conta o link cai na
lista da pessoa, que é o que a torna útil no dia a dia.

- [ ] Manifest V3 (Chrome/Edge) e WebExtensions (Firefox) a partir do mesmo código
- [ ] Ação do ícone: URL da aba ativa → `POST /v1/shorten` → clipboard → badge de confirmação
- [ ] Menu de contexto ("encurtar este link"), além da página atual
- [ ] Publicar nas duas lojas

🎯 Encurtar a aba atual sem sair dela, com o link já copiado.

**Permissão mínima é decisão de design, não detalhe.** `activeTab` dá acesso à aba **apenas
no clique**, e é tudo que esta extensão precisa. Pedir `host_permissions: ["<all_urls>"]` —
o caminho mais fácil — é pedir para ler qualquer página que a pessoa visite: atrasa a
revisão das lojas e transforma um comprometimento da extensão num vazamento do histórico
inteiro. `activeTab` + `clipboardWrite`, e nada mais.

**Duas coisas que precisam mudar no servidor:**

1. **CORS.** Hoje não existe, porque o front é mesma origem e nunca precisou. A extensão
   chama de `chrome-extension://…`, que é outra origem. Ou a API ganha um `CorsLayer` em
   `/v1/shorten`, ou a extensão declara `host_permissions` para `oxid.uk` e faz o fetch do
   service worker, que escapa do CORS. A primeira é a honesta; a segunda troca configuração
   de servidor por permissão mais ampla no cliente.
2. **O rate limit por IP fica frágil.** Ele existe para conter abuso de escrita, e a
   extensão multiplica escritas por pessoa. Em rede com NAT todo mundo divide o IP e um
   usuário ativo consome a cota dos outros — o mesmo problema de chave compartilhada já
   visto no `X-Forwarded-For` da Etapa 5. Com a Etapa 11 pronta, limitar por conta quando
   houver sessão e por IP só no caminho anônimo.

**A credencial da extensão não é o cookie de sessão.** Extensão não compartilha cookie com o
site de forma confiável entre navegadores. O caminho é o token de API que ficou como
alternativa preterida na Etapa 11: a pessoa gera um, cola na extensão, e ele vale como
credencial daquele cliente — revogável sem derrubar a sessão do navegador.

## Etapa 14 — Painel do administrador

Uma página autenticada com os números do produto — criados nas últimas 24 h, total
acumulado, top códigos por acesso, taxa de erro — ao lado dos números de sistema que a
Etapa 7 já coleta.

Depois das Etapas 11 e 12: sem sessão não há como restringir o acesso, e sem os eventos de
clique metade dos números do produto não existe.

- [ ] Papel de administrador em `users` — sem isso, "autenticado" viraria "qualquer conta"
- [ ] `GET /v1/admin/stats`: criados por período, total, taxa de erro, top códigos
- [ ] Página no front, atrás de sessão com esse papel
- [ ] Números de sistema vindos do Prometheus, não recalculados no Postgres

🎯 Responder "quantos links nas últimas 24 h" e "onde está o gargalo" numa tela só.

**A separação que não pode ser perdida.** São duas fontes com naturezas diferentes: os
números de produto saem do Postgres/ClickHouse (verdade transacional, tem de ser exata) e os
de sistema saem do Prometheus (série temporal amostrada, aproximada por construção). Um
painel que mistura as duas como se fossem a mesma coisa mente nas duas — cada uma tem que
ser lida de onde vive, e ficar claro qual é qual.

**Cuidado com `COUNT(*)` nas últimas 24 h.** Numa tabela projetada para 365 bilhões de
linhas, essa consulta varre índice a cada carregamento do painel. Vira contador incremental
ou tabela de agregado diário — a mesma decisão de rollup que a Etapa 12 já enfrenta.

**Grafana já resolve os números de sistema.** Este painel existe para o que ele não tem:
dado de produto. Reimplementar gráfico de latência aqui seria trabalho duplicado com
resultado pior — o caminho é embutir o painel do Grafana ou apontar para ele.

---

## SonarQube no CI ✅

- [x] Job no `ci.yml` com `SonarSource/sonarqube-scan-action`
- [x] `fetch-depth: 0` no checkout — sem histórico completo o Sonar não calcula *new code*
- [x] `sonar-project.properties`: `sonar.sources=crates`, excluindo `target/`, `.sqlx/`,
      `dist/` e `fonts/`
- [x] Importar o que já temos em vez de duplicar análise: `cargo clippy
      --message-format=json` e LCOV do `cargo llvm-cov` (medido dentro do job de testes, que
      já tem Postgres e Redis de pé)
- [x] Scan pulado em PR de fork, onde o `SONAR_TOKEN` não existe — falharia sem dizer o
      motivo
- [ ] Adicionar `SONAR_TOKEN` em Settings → Secrets → Actions
- [ ] Confirmar o `projectKey` depois de importar o projeto (`organization` já está
      confirmada: é a mesma de `josemoura212/FC-sonar-node`)

**Por que aqui precisa de token, e no `Fc-sonar` não precisou.** Aquele repositório usa
*Automatic Analysis*, em que o SonarQube Cloud lê o repositório pelo app do GitHub, sem CI e
sem segredo. Esse modo cobre todas as linguagens **exceto Objective-C, Dart e Rust** — e não
importa cobertura nem relatório de linter externo. Ou seja: para este projeto ele não
funcionaria, e mesmo se funcionasse deixaria de fora justamente o clippy e o LCOV. O
`FC-sonar-node` já usa o caminho com scanner e token, que é o adotado aqui.

**Por que importar em vez de deixar o Sonar analisar por conta:** o portão de lint do CI já
é a afirmação mais rígida sobre este código — clippy com `pedantic` em deny. Se o Sonar
reportasse critério próprio, passariam a existir dois padrões em desacordo.

## Pendência de infra — superfície de rede do nó

Revisada em 2026-07-27, ao publicar o Grafana e notar que os serviços do cluster
respondem direto no nó, por fora do proxy: sem TLS, sem os headers de segurança e
sem o rate limit da CDN. Num painel com formulário de login isso é pior — a senha
viaja em claro.

Reduzir o que o nó aceita de fora ao mínimo, de modo que todo tráfego entre pelo
caminho pretendido em vez de por atalhos.

- [x] Revisar as regras de entrada na **Security List da Oracle**, não no
      `iptables` local: o k3s reescreve regras de iptables e a alteração some num
      restart. O proxy roda no mesmo host e alcança os serviços por dentro, então
      fechar a porta de fora não quebra o caminho normal
- [x] Remover a **faixa** de portas de serviço. Enquanto era faixa, todo Service
      novo nascia público sem ninguém decidir isso — e nenhum deles precisava
      dela, porque o proxy chega por dentro
- [ ] Restringir a entrada HTTP/HTTPS aos **ranges da Cloudflare**
      (`cloudflare.com/ips`), para que a origem só aceite tráfego vindo da CDN.
      Hoje alcançar a origem por IP com o `Host` certo pula a CDN inteira.
      Todo hostname roteado já está atrás do proxy, inclusive para o desafio ACME,
      então a restrição não tira caminho de ninguém
- [ ] Restringir o **plano de controle do cluster** a origens conhecidas. É a
      porta que entrega o cluster, não uma aplicação — e a única cuja exposição
      não é compensada por nada mais na pilha. Presa ao modelo de deploy: hoje o
      CI empurra de fora, e restringir por origem exigiria runner próprio ou
      inverter para o cluster puxar
- [ ] Fechar as portas dos **painéis de administração**, que respondem por porta
      própria contornando o proxy — sendo que o mesmo painel já é servido por
      domínio, com TLS e os headers. A porta direta é só a versão sem nenhum deles

**Metade das regras não tinha processo do outro lado.** O levantamento cruzou o
que a Security List abria com o que de fato escutava no host: regras de um serviço
já desinstalado e de aplicações que só publicam dentro da rede do Docker. Uma
regra órfã não é inócua — é uma porta esperando alguém subir algo naquele número.

**A Security List é a única camada, e isso muda o peso de cada regra.** Vários
serviços do host escutam em `0.0.0.0` (kubelet, exporter de nó, o proxy de um
banco) e estão fora da internet só porque nenhuma regra os alcança. Não há
firewall de host segurando nada, pelo motivo já dito: o k3s reescreve o iptables.
Cada regra aberta é a exposição inteira, sem segunda linha.

O que **não** está exposto, e vale registrar: a porta das métricas não responde de
fora (timeout confirmado). O listener separado da Etapa 7 fez o trabalho dele.

**Endereços não vão neste arquivo.** O repositório é público, e escrever aqui qual
porta responde em qual endereço economiza reconhecimento para quem procurar. Os
números concretos vivem na Security List e nos manifests; este bloco registra a
decisão, não o alvo.

**Cloudflare Tunnel — depois da Etapa 10, não antes.** Ele é melhor: zero portas
de entrada e IP de origem nunca exposto. Mas fecha o caminho que as Etapas 9 e 10
precisam, porque o k6 tem que medir a origem sem a CDN no meio — medir através
dela mediria a CDN. Com tudo fechado, o gerador de carga teria que rodar dentro
da VPS, disputando CPU com o alvo. É o erro que o estudo original cometeu e que
este projeto existe para não repetir.

**O que a redação não resolve.** Tirar endereços daqui impede reintroduzi-los e
para de apontar o alvo, mas não recupera o que já esteve num repositório público
nem o que o DNS entrega de graça. O controle é a regra de firewall; a redação é
higiene. Enquanto os itens acima não fecharem, o endereço deve ser tratado como
conhecido.

## Pendência de CI — quem segura um deploy não revisado

**O que foi tentado e revertido:** disparar o deploy em `pull_request` com `types: [closed]`,
para que push direto na `main` (possível com o bypass de admin) não publicasse nada. Falha
porque runs de `pull_request` **vindos de fork não recebem secrets** — um PR da comunidade,
depois de mergeado, quebraria por falta de `KUBECONFIG`. O repo é público; era questão de
tempo.

**O que ficou:** `push` na `main`, que roda no repositório base e sempre tem os secrets. Com o
ruleset, a única forma de a `main` andar é um PR mergeado, então o gatilho já descreve a
realidade.

**O que falta:** o gatilho nunca foi o lugar certo para barrar deploy não revisado — quem faz
isso é o **environment**. O job de deploy já declara `environment: production`; com *required
reviewers* configurado, ele fica pendente esperando aprovação humana, **inclusive num push de
bypass**. É mais forte do que o gatilho conseguia ser.

- [ ] Configurar *required reviewers* no environment `production` (Settings → Environments)
- [ ] Confirmar que um push de bypass fica pendente de aprovação em vez de publicar

**Descartado:** `pull_request_target` roda no contexto base *com* secrets — fazer checkout do
código do fork ali entrega as credenciais a quem abriu o PR. `workflow_run` resolveria, mas
acrescenta um workflow inteiro para um problema que o environment dissolve.

---

## Backlog pós-v1 (não começar antes da Etapa 10)

- [ ] Alta disponibilidade: 2º Nginx com keepalived/VRRP, Postgres standby com
      failover (Patroni/repmgr), Redis Sentinel (proteção contra thundering herd)
- [ ] Particionamento por tempo no Postgres (retenção de 10 anos = drop de partição)
- [ ] Sharding por hash do shortcode quando o volume justificar
- [ ] Write path: `synchronous_commit = off`, batch de inserts
- [x] ~~302 + analytics como modo opcional~~ — promovido a Etapa 12, com o 302 restrito
      às URLs que têm dono
