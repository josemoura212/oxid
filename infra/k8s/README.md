# Kubernetes manifests

Enough to run oxid on any cluster. They are the manifests this project actually
deploys with, not a sketch — but they stop where every environment starts to
differ.

| File | What it is |
|---|---|
| `00-namespace.yaml` | the `oxid` namespace |
| `10-postgres.yaml` | StatefulSet + headless Service + PVC |
| `20-redis.yaml` | Deployment + Service, no volume — the cache is disposable |
| `30-api.yaml` | API Deployment, ConfigMap and Service |
| `40-deploy-access.yaml` | ServiceAccount, Role and RoleBinding for CI |
| `50-migrate-job.yaml` | migration Job, templated on `IMAGE_REF` |
| `60-web.yaml` | front end Deployment and Service |
| `monitoring/` | PodMonitor and Grafana, for a cluster already running Prometheus |
| `mint-kubeconfig.sh` | builds a kubeconfig for the CI ServiceAccount |

## Applying

```bash
kubectl apply -f infra/k8s/00-namespace.yaml

# The database secret. Never in a versioned file.
kubectl -n oxid create secret generic oxid-db \
  --from-literal=username=oxid \
  --from-literal=database=oxid \
  --from-literal=password="$(openssl rand -hex 24)"

kubectl apply -f infra/k8s/10-postgres.yaml
kubectl -n oxid rollout status statefulset/postgres

kubectl apply -f infra/k8s/20-redis.yaml
kubectl apply -f infra/k8s/30-api.yaml
kubectl apply -f infra/k8s/60-web.yaml
```

The image has to exist before the API is applied. Either let the `Deploy`
workflow publish it, or push one by hand from `infra/Dockerfile`.

## What you will want to change

- **`APP_APPLICATION__BASE_URL`** in `30-api.yaml` — the shortener puts this in
  every response it generates, so a wrong value ships broken links.
- **`nodePort`** on the API and front end Services. They are `NodePort` because
  the proxy in this setup lives outside the cluster; with an ingress controller
  inside it, `ClusterIP` plus an Ingress is the better shape.
- **Storage class** on the Postgres PVC. It relies on the cluster default.
- **`replicas`** on the API. Two by default, and worth reading `ROADMAP.md`
  stage 8 before assuming that means twice the throughput — on a single node it
  does not.

## Migrations run as a Job, never at boot

With more than one replica, every pod would race to migrate the same schema. The
Job runs `oxid-migrate` from the **same image** as the API, so the schema and the
code expecting it are always the same build.

```bash
kubectl -n oxid delete job oxid-migrate --ignore-not-found
sed "s|IMAGE_REF|ghcr.io/your/image:tag|" infra/k8s/50-migrate-job.yaml \
  | kubectl apply -f -
kubectl -n oxid logs job/oxid-migrate -f
```

## Metrics are on a private port

`/metrics` is not a route on the public router. It answers on a separate listener
declared in the Deployment and **absent from the Service**, so nothing outside
the cluster can reach it. Publishing request volume, latency distribution and
cache behaviour to anyone who asks is a bigger giveaway than it looks.

That is also why `monitoring/10-podmonitor.yaml` is a `PodMonitor` and not a
`ServiceMonitor`: a ServiceMonitor would need a second Service carrying the
metrics port, which would create the path this deliberately avoids.

## What is not here, on purpose

Proxy routes, hostnames, TLS wiring, node tuning and capacity measurements. Every
deployment differs exactly there, and someone else's wiring is noise rather than a
starting point — it looks authoritative while describing a machine you do not
have.

If you run the proxy outside the cluster, you need one route per Service pointing
at the NodePort. If you run an ingress controller inside it, you want Ingress
objects and no NodePort at all. Neither is more correct; they are different
environments.

## Deploys change the image, not the manifest

`.github/workflows/deploy.yml` uses `kubectl set image` **by digest**, which
touches only the container image — a tag cannot tell you what is running nor what
to roll back to. Change anything else here (probes, resources, securityContext)
and it takes a manual `kubectl apply`; the workflow will not pick it up.
