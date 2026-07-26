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

## Etapa 3 — Persistência com sqlx

- [ ] Postgres local via docker-compose (`infra/docker-compose.yml`)
- [ ] Migration: tabela `urls` (id bigserial PK, url_hash unique, long_url,
      short_code unique nullable até gerar, created_at)
- [ ] `PgPool` no `AppState`, criado no main com `max_connections` vindo de config
- [ ] `repo.rs`: inserir com `ON CONFLICT DO NOTHING RETURNING id` + SELECT fallback
      (idempotência no banco, não na aplicação)
- [ ] `sqlx::query_as!` compilando contra o schema real

🎯 Inserir a mesma URL duas vezes retorna o MESMO id, provado por teste de integração.
🦀 async com banco, macros do sqlx, `DATABASE_URL` em `.env`, `Result` + `?`.

## Etapa 4 — Rotas de escrita e leitura

- [ ] `error.rs`: enum `AppError` (NotFound, InvalidUrl, Db, Cache...)
      com `impl IntoResponse` mapeando para status corretos
- [ ] `POST /v1/shorten`: `Json<ShortenRequest>`, validação http/https (crate `url`),
      fluxo id → obfuscate → base62 → persistir short_code → responder
- [ ] `GET /v1/urls/{code}`: `Path<String>`, resolver e `Redirect::permanent` (301)
- [ ] 404 limpo para código inexistente, 400 para URL inválida

🎯 Fluxo completo via curl: encurtar → seguir redirect → chegar na URL original.
🦀 `Deserialize`/`Serialize` com serde, `?` propagando para `AppError`, `IntoResponse`.

## Etapa 5 — Cache Redis (cache-aside + cache negativo)

- [ ] Redis no docker-compose com `maxmemory` definido e `allkeys-lru`
- [ ] Cliente Redis no `AppState`
- [ ] `cache.rs`: no GET, tentar cache → miss → banco → gravar no cache SEMPRE (sem TTL)
- [ ] Popular o cache também na escrita (o dado acabou de nascer quente)
- [ ] Cache negativo: código inexistente grava sentinela com **SET NX + TTL curto**
- [ ] Contadores de hit/miss (mesmo que só log por enquanto)

🎯 Segunda leitura do mesmo código não toca o Postgres (provar por log/métrica).
🎯 Teste da corrida: escrita concorrente positiva nunca é sobrescrita por negativa.
🦀 Traits de cliente async, serialização para o cache, TTL condicional.

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

## Backlog pós-v1 (não começar antes da Etapa 10)

- [ ] Alta disponibilidade: 2º Nginx com keepalived/VRRP, Postgres standby com
      failover (Patroni/repmgr), Redis Sentinel (proteção contra thundering herd)
- [ ] Particionamento por tempo no Postgres (retenção de 10 anos = drop de partição)
- [ ] Sharding por hash do shortcode quando o volume justificar
- [ ] Write path: `synchronous_commit = off`, batch de inserts
- [ ] 302 + analytics como modo opcional (trade-off vs cache do navegador com 301)
