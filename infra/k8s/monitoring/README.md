# Métricas de produção

Como o Prometheus do cluster enxerga o oxid, e como o Grafana é publicado.

## O que já existia

O cluster tem **kube-prometheus-stack** no namespace `monitoring` desde antes
deste projeto: operator, Prometheus, kube-state-metrics e node-exporter. Não foi
preciso instalar Prometheus nenhum — só dizer a ele onde raspar. Grafana **não**
vinha junto; é o que o `20-grafana.yaml` acrescenta.

## 1. Fazer o Prometheus enxergar o oxid

```bash
kubectl apply -f infra/k8s/monitoring/10-podmonitor.yaml
```

Duas coisas precisam estar certas, ou o arquivo é ignorado **em silêncio**:

- **`release: prometheus` nos labels.** Aquele Prometheus seleciona monitores por
  esse label exato. Sem ele, nada acontece e nada avisa.
- **A porta precisa estar declarada no pod.** O `PodMonitor` casa pelo *nome*
  (`metrics`), e o Deployment em produção não a declarava: o workflow de deploy
  usa `kubectl set image`, que troca só a imagem e ignora o resto do manifest.

  ```bash
  kubectl -n oxid patch deployment api --type=json \
    -p '[{"op":"add","path":"/spec/template/spec/containers/0/ports/-","value":{"containerPort":9090,"name":"metrics","protocol":"TCP"}}]'
  ```

  A porta já estava em `infra/k8s/30-api.yaml`; faltava chegar ao cluster. Um
  `kubectl apply -f infra/k8s/30-api.yaml` também resolve, ao custo de trocar a
  imagem por digest de volta pela tag `latest`.

Conferir:

```bash
kubectl -n monitoring port-forward svc/prometheus-kube-prometheus-prometheus 9090:9090
curl -s 'http://127.0.0.1:9090/api/v1/query?query=sum%20by%20(route,status)%20(http_requests_total)'
```

## 2. Grafana

A senha nunca entra em arquivo:

```bash
kubectl -n monitoring create secret generic grafana-admin \
  --from-literal=password="$(openssl rand -base64 24)"
```

Lê-la depois, sem exibir na tela:

```bash
kubectl -n monitoring get secret grafana-admin \
  -o jsonpath='{.data.password}' | base64 -d | xclip -selection clipboard
```

O dashboard entra como ConfigMap, a partir do mesmo arquivo que o compose local
usa — um arquivo para os dois ambientes:

```bash
kubectl -n monitoring create configmap grafana-dashboards \
  --from-file=oxid.json=infra/grafana/provisioning/dashboards/oxid.json

sed "s|GRAFANA_HOST|grafana.exemplo.com|" infra/k8s/monitoring/20-grafana.yaml \
  | kubectl apply -f -

kubectl -n monitoring rollout status deployment/grafana
```

Depois de editar o dashboard:

```bash
kubectl -n monitoring create configmap grafana-dashboards \
  --from-file=oxid.json=infra/grafana/provisioning/dashboards/oxid.json \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl -n monitoring rollout restart deployment/grafana
```

## 3. Publicar só o Grafana

Troque `GRAFANA_HOST` em `30-traefik-grafana.yaml` pelo hostname real e cole no
Coolify (Servers → Proxy → Dynamic Configurations) como `grafana.yaml`.

**Por que o hostname é um placeholder:** a Cloudflare serve este domínio com
certificado *wildcard*, então subdomínios não aparecem nos logs de Certificate
Transparency — ao contrário de um certificado por host do Let's Encrypt, que
publica todo nome que assina. Versionar o nome real entregaria de graça algo que
não se descobre de outro jeito. É uma camada fina, e não é ela que protege o
Grafana: quem protege é o login e o firewall.

**Prometheus fica de fora de propósito.** Ele não tem autenticação nenhuma:
publicá-lo seria entregar todas as métricas a quem descobrisse o hostname.

## Pendências

- **NodePorts abertos na internet** (30091, 30092, 30093). Respondem direto pelo
  IP do nó, contornando Cloudflare e Traefik — sem TLS e sem os headers de
  segurança. No caso do Grafana isso significa login em HTTP puro. Fechar no
  firewall da Oracle é o item mais urgente aqui.
- Retenção do Prometheus não foi revisada; o padrão do chart costuma ser curto
  demais para comparar duas rodadas de teste de carga separadas por dias.
- Nenhum alerta configurado. Métrica que ninguém olha só serve depois do
  incidente.
- O dashboard soma as duas réplicas. "Uma réplica está pior que a outra" é
  invisível hoje.
