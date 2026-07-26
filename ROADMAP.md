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
- [ ] CSS inline via `data-trunk rel="inline"` — elimina os 150 ms de bloqueio de
      renderização por um round-trip de 1,5 KB
- [ ] Headers de segurança no Traefik: HSTS, `frame-ancestors`, COOP

🎯 Acessibilidade em 100 e as três auditorias de segurança baratas fechadas.
   Falta rodar o Lighthouse de novo depois do deploy para confirmar o 100.

**Fora desta etapa, de propósito:** CSP (o trunk emite o bootstrap do wasm como script
inline, então exige hash por build ou nonce — trabalho de verdade) e markup estático/SSR
para melhorar FCP. Os dois estão em `docs/PERFORMANCE-WEB.md` com o custo estimado.

**Não é problema nosso:** os avisos de cache de 5 KiB e parte dos "3 KiB de JS" vêm do
script que a Cloudflare injeta no `<body>` (`max-age=300`, verificado em produção).
Some desligando *Bot Fight Mode* — decisão de segurança, não de performance.

## Etapa 6 — Configuração e dimensionamento

- [ ] Toda config via env: porta, DATABASE_URL, REDIS_URL, tamanho do pool
- [ ] `.env.example` documentado
- [ ] Pool do Postgres pequeno por padrão (regra: cores do banco × 2)
- [ ] Timeouts explícitos: acquire do pool, statement, conexão Redis

🎯 App sobe em ambiente limpo só com `.env` preenchido; pool visível nas métricas.
🦀 Structs de config com serde/envy, `Duration`, fail-fast no bootstrap.

## Etapa 7 — Observabilidade

- [ ] `metrics` + `metrics-exporter-prometheus`, endpoint `/metrics`
- [ ] Histogramas de latência por rota, contadores hit/miss, gauge do pool
- [ ] Prometheus + Grafana no docker-compose scrapando app, postgres exporter,
      redis exporter, nginx exporter
- [ ] Dashboard mínimo: p50/p95/p99 por rota, hit rate, conexões do pool, CPU por serviço

🎯 Dá para responder "onde está o gargalo?" olhando um único dashboard.
🦀 Instrumentação com spans do tracing, macros de métricas, custo de instrumentar.

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

---

## Próxima PR — SonarQube no CI

Combinado em 2026-07-26, **antes da Etapa 7**. O Sonar já está configurado do lado do
GitHub; o que falta é o repositório.

- [ ] Confirmar SonarQube Cloud × Server, `projectKey` e `organization`
- [ ] Job no `ci.yml` com `SonarSource/sonarqube-scan-action` e `SONAR_TOKEN` nos secrets
- [ ] `fetch-depth: 0` no checkout — sem histórico completo o Sonar não calcula *new code*
- [ ] `sonar-project.properties`: `sonar.sources=crates`, excluindo `target/`, `.sqlx/` e
      `crates/oxid-web/dist/`
- [ ] Importar o que já temos em vez de duplicar análise: `cargo clippy
      --message-format=json` em `sonar.rust.clippy.reportPaths` e LCOV
      (`cargo llvm-cov`) em `sonar.rust.lcov.reportPaths`

⚠️ O scan em PR de fork esbarra na **mesma falta de secret** do deploy — resolver junto com
a pendência abaixo, não separado.

## Pendência de CI — deploy sem secret em PR de fork

**O problema:** o deploy passou a disparar em `pull_request` com `types: [closed]`, para que
push direto na `main` (possível com o bypass de admin) não publique nada. Só que runs de
`pull_request` **vindos de fork não recebem secrets** — sem `KUBECONFIG`, o deploy falha. O
repo é público, então é questão de tempo até alguém abrir PR de fork.

Hoje o contorno é `workflow_dispatch` na mão. As saídas de verdade, em ordem de preferência:

1. **Voltar para `push: branches: [main]` e proteger o `environment: production`.**
   Depois do merge o evento é um push no repositório base, que **recebe secrets normalmente**
   — o problema simplesmente não existe. E o objetivo de "não publicar sem revisão" passa a
   ser feito por quem tem essa função: *required reviewers* no environment, que segura o job
   até alguém aprovar, inclusive num push de bypass. O job já declara
   `environment: production`; falta só configurar a regra no GitHub.
2. **`workflow_run`** encadeado depois do CI. Roda no contexto do repo base, com secrets, e
   usa a definição do workflow que está na branch default. Resolve, mas acrescenta um
   workflow e um nível de indireção para um problema que a opção 1 dissolve.
3. **`pull_request_target`** — **não usar.** Ele roda no contexto base *com* secrets e é o
   antipadrão clássico de CI: fazer checkout do código do fork ali entrega os segredos a
   quem abriu o PR.

- [ ] Configurar *required reviewers* no environment `production`
- [ ] Voltar o gatilho do deploy para `push` na `main`
- [ ] Confirmar que um push de bypass fica pendente de aprovação em vez de publicar

---

## Backlog pós-v1 (não começar antes da Etapa 10)

- [ ] Alta disponibilidade: 2º Nginx com keepalived/VRRP, Postgres standby com
      failover (Patroni/repmgr), Redis Sentinel (proteção contra thundering herd)
- [ ] Particionamento por tempo no Postgres (retenção de 10 anos = drop de partição)
- [ ] Sharding por hash do shortcode quando o volume justificar
- [ ] Write path: `synchronous_commit = off`, batch de inserts
- [x] ~~302 + analytics como modo opcional~~ — promovido a Etapa 12, com o 302 restrito
      às URLs que têm dono
