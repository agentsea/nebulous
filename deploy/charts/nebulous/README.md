# nebulous

![Version: 0.2.9](https://img.shields.io/badge/Version-0.2.9-informational?style=flat-square) ![Type: application](https://img.shields.io/badge/Type-application-informational?style=flat-square) ![AppVersion: 0.1.88](https://img.shields.io/badge/AppVersion-0.1.88-informational?style=flat-square)

A cross-cloud container orchestrator for AI workloads

## Quickstart

Generate a random 32 byte, base64 encoded key:
```bash
openssl rand -base64 32
# or
python3 -c "import base64, os; print(base64.b64encode(os.urandom(32)).decode())"
```

Create a `values.yaml` file and add the key:
```yaml
encryptionKey:
  encodedValue: "<base64 encoded key>"
```

Add the Tailscale API key and auth key:
```yaml
tailscale:
  apiKey: <Tailscale API key>
  authKey: <Tailscale auth key for Nebulous>
```

The integrated Redis database requires an auth key for Tailscale as well:
```yaml
redis:
  create: true
  tailscale:
    authKey: <Tailscale auth key for Redis>
```

Finally, enable the creation of the integrated Postgres database:
```yaml
postgres:
  create: true
```

Add the nebulous chart repository and install the chart into a dedicated namespace:

```bash
helm repo add nebulous https://agentsea.github.io/nebulous
helm install nebulous nebulous/nebulous -f values.yaml \
  --namespace nebulous --create-namespace
```

## Credential secrets

In production, the encryption key and Tailscale keys should be provided as Kubernetes secrets
and not as Helm chart values.

You can use the following template to create them.
This template assumes installation in the `nebulous` namespace
and the secret names and keys as defined in the Helm chart's default [values.yaml](./values.yaml).

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: nebulous-secret
  namespace: nebulous
data:
  ENCRYPTION_KEY: <base64 encoded key>
---
apiVersion: v1
kind: Secret
metadata:
  name: tailscale-secret
  namespace: nebulous
stringData:
  API_KEY: "<Tailscale API key>"
  AUTH_KEY: "<Tailscale auth key for Nebulous>"
---
apiVersion: v1
kind: Secret
metadata:
  name: tailscale-redis-secret
  namespace: nebulous
data:
  AUTH_KEY: "<Tailscale auth key for Redis>"
```

## Values

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| bucket.auth | object | `{"accessKeyId":"","secretAccessKey":""}` | Manual configuration of the AWS credentials. Not recommended for production. |
| bucket.name | string | `""` | The name of the Amazon S3 bucket to use for Nebulous. |
| bucket.region | string | `""` | The region of the Amazon S3 bucket to use for Nebulous. |
| bucket.secret.keys.accessKeyId | string | `"AWS_ACCESS_KEY_ID"` | The key in the secret containing the access key ID. |
| bucket.secret.keys.secretAccessKey | string | `"AWS_SECRET_ACCESS_KEY"` | The key in the secret containing the secret access key. |
| bucket.secret.name | string | `"aws-secret"` | The name of the secret containing the AWS credentials. |
| encryptionKey.encodedValue | string | `""` | The 32 byte encryption key encoded in base64. Not recommended for production. |
| encryptionKey.secret.keys.encryptionKey | string | `"ENCRYPTION_KEY"` | The key in the secret containing the encryption key. |
| encryptionKey.secret.name | string | `"nebulous-secret"` | The name of the secret containing the 32 byte encryption key. |
| extraEnv | list | `[]` | Additional environment variables to pass to the Nebulous server container. |
| headscale.create | bool | `false` | If true, create a Headscale deployment and service. Overrides tailscale configuration. Not recommended for production. |
| headscale.derp | object | `{"configMap":{"key":"servers.yaml","name":""},"externalMaps":[]}` | The Headscale DERP configuration. Either 'externalMapUrls' or 'configMap' must be set. |
| headscale.derp.configMap.key | string | `"servers.yaml"` | The key in the ConfigMap containing the DERP server configuration YAML file. |
| headscale.derp.configMap.name | string | `""` | The name of the ConfigMap containing the DERP server configuration. |
| headscale.derp.externalMaps | list | `[]` | URLs of externally available DERP maps encoded in JSON. |
| headscale.dns.baseDomain | string | `""` | The base domain for MagicDNS hostnames. Cannot be the same as the Headscale server's domain. Refer to https://github.com/juanfont/headscale/blob/main/config-example.yaml for details. |
| headscale.domain | string | `""` | The domain under which the Headscale server is exposed. Required if create is true. The headscale server must be reachable at https://${domain}:443. |
| headscale.imageTag | string | `"stable"` | The Headscale image tag. |
| headscale.ingress.annotations | object | `{}` | Annotations to add to the Ingress resource. |
| headscale.ingress.enabled | bool | `false` | If enabled, create an Ingress resource. Ignored unless 'enabled' is true. |
| headscale.ingress.ingressClassName | string | `""` | The ingress class. |
| headscale.log.format | string | `"text"` | The log format of the Headscale server. Options are "text" or "json". |
| headscale.log.level | string | `"info"` | The log level of the Headscale server. Options are "off", "trace", "debug", "info", "warn", "error". |
| headscale.namespaceOverride | string | `""` | Namespace override for the Headscale deployment. |
| headscale.persistence.size | string | `"1Gi"` | The size of the PersistentVolumeClaim for the Headscale data. |
| headscale.persistence.storageClassName | string | `""` | The storage class of the PersistentVolumeClaim for the Headscale data. |
| headscale.prefixes | object | `{"v4":"100.64.0.0/10","v6":"fd7a:115c:a1e0::/48"}` | Prefixes to allocate tailaddresses from. Must be within the IP ranges supported by the Tailscale client. Refer to https://github.com/juanfont/headscale/blob/main/config-example.yaml for details. |
| headscale.resources | object | `{}` | The resource requests and limits for the headscale container. |
| headscale.service.annotations | object | `{}` | The annotations to add to the Kubernetes service. |
| headscale.service.nameOverride | string | `""` | Override the name of the Kubernetes service. |
| headscale.service.port | int | `80` | The port of the Kubernetes service. |
| headscale.service.type | string | `"ClusterIP"` | The type of the Kubernetes service. Options are "ClusterIP", "NodePort", and "LoadBalancer". |
| headscale.tls.letsencrypt.email | string | `""` | The email address for the Let's Encrypt certificate. |
| headscale.tls.letsencrypt.hostname | string | `""` | The hostname for the Let's Encrypt certificate. Has to match the domain of the Headscale server. |
| image.pullPolicy | string | `"IfNotPresent"` |  |
| image.repository | string | `"us-docker.pkg.dev/agentsea-dev/nebulous/server"` | The repository to pull the server image from. |
| image.tag | string | `""` | The nebulous image tag. Defaults to the Helm chart's appVersion. |
| ingress.annotations | object | `{}` | Annotations to add to the Ingress resource. |
| ingress.enabled | bool | `false` | If enabled, create an Ingress resource. |
| ingress.ingressClassName | string | `""` | The ingress class. |
| local.enabled | bool | `false` | If enabled, nebulous can run Pods in the local cluster. |
| logLevel | string | `"info"` | The log level of the Nebulous server. Options are "off", "trace", "debug", "info", "warn", "error". |
| messageQueue.type | string | `"redis"` | The message queue type. The currently only supported value is "redis". |
| namespaceOverride | string | `""` | Override the namespace. By default, Nebulous is deployed to the Helm release's namespace. |
| openmeter.enabled | bool | `false` | Enable usage monitoring with OpenMeter. |
| openmeter.secret.keys.token | string | `"TOKEN"` | The key in the eecret containing the OpenMeter API token. |
| openmeter.secret.name | string | `"openmeter-secret"` | The name of the secrets containing the OpenMeter API token. |
| openmeter.token | string | `""` | The OpenMeter API token. Not recommended for production. |
| openmeter.url | string | `"https://openmeter.cloud"` | The URL to report OpenMeter data to. |
| orign.url | string | `""` | The URL that Nebulous uses to connect to the Orign server. |
| postgres.auth | object | `{"database":"nebulous","host":"","password":"nebulous","port":5432,"user":"nebulous"}` | Manual configuration of the Postgres connection. Except for 'host', this information is also used if 'create' is true. |
| postgres.create | bool | `false` | If enabled, create a Postgres deployment and service. Not recommended for production. |
| postgres.imageTag | string | `"17"` | The postgres image tag. Ignored unless 'create' is true. |
| postgres.persistence.size | string | `"100Gi"` | The size of the PersistentVolumeClaim for the Postgres data. |
| postgres.persistence.storageClassName | string | `""` | The storage class of the PersistentVolumeClaim for the Postgres data. |
| postgres.resources | object | `{}` | The resource requests and limits for the Postgres container. |
| postgres.secret.keys.connectionString | string | `"CONNECTION_STRING"` | The key in the secret containing the Postgres connection string. |
| postgres.secret.name | string | `"postgres-secret"` | Name of the secret with the Postgres connection string. |
| providers.runpod.auth | object | `{"apiKey":"","containerRegistryAuthId":""}` | Manual configuration of the Runpod credentials. Not recommended for production. |
| providers.runpod.enabled | bool | `false` | Enable access to Runpod. |
| providers.runpod.secret.keys.apiKey | string | `"RUNPOD_API_KEY"` | The key in the secret containing the API key. |
| providers.runpod.secret.keys.containerRegistryAuthId | string | `"RUNPOD_CONTAINER_REGISTRY_AUTH_ID"` | The key in the secret containing the container registry auth ID. |
| providers.runpod.secret.name | string | `"runpod-secret"` | The name of the secret containing the Runpod credentials. |
| publicUrl | string | `""` | The URL that agents use to connect to Nebulous. |
| redis.auth | object | `{"database":0,"host":"","password":"nebulous","port":6379}` | Manual configuration of the Redis connection. Except for 'host', this information is also used if 'create' is true. |
| redis.create | bool | `false` | If enabled, create a Redis deployment and service. Not recommended for production. |
| redis.imageTag | string | `"8"` | The redis image tag. Ignored unless 'create' is true. |
| redis.persistence.acl.size | string | `"64Mi"` | The size of the PVC for the Redis ACL file. |
| redis.persistence.acl.storageClassName | string | `""` | The storage class of the PersistentVolumeClaim for the Redis ACL file. |
| redis.persistence.data.size | string | `"5Gi"` | The size of the PVC for the Redis data, |
| redis.persistence.data.storageClassName | string | `""` | The storage class of the PersistentVolumeClaim for the Redis data. |
| redis.persistence.enabled | bool | `false` | If enabled, persist the Redis data. |
| redis.resources | object | `{}` | The resource requests and limits for the Redis container. |
| redis.secret.keys.connectionString | string | `"CONNECTION_STRING"` | The key in the secret containing the Redis connection string. |
| redis.secret.keys.password | string | `"PASSWORD"` | The key in the secret containing the Redis password. |
| redis.secret.name | string | `"redis-secret"` | Name of the secret with the Redis connection string and password. |
| redis.service.annotations | object | `{}` | The annotations to add to the Kubernetes service. |
| redis.service.nameOverride | string | `""` | Override the name of the Kubernetes service. |
| redis.serviceAccountName | string | `"redis"` | The name of the Kubernetes service account for the Redis Pod. |
| redis.tailscale.authKey | string | `""` | The Tailscale auth key for Redis. If headscale.enabled is true, this is ignored. |
| redis.tailscale.resources | object | `{}` | The resource requests and limits for the Redis database's Tailscale sidecar container. |
| redis.tailscale.secret.keys.authKey | string | `"AUTH_KEY"` | The key in the secret containing the Tailscale auth key. |
| redis.tailscale.secret.name | string | `"tailscale-redis-secret"` | Name of the secret with the Tailscale auth key for Redis. |
| resources | object | `{}` | The resource requests and limits for the Nebulous server container. |
| rootOwner | string | `"agentsea"` | The owner of the Nebulous root. |
| service.annotations | object | `{}` | Annotations to add to the Kubernetes service. |
| service.nameOverride | string | `""` | Override the name of the Kubernetes service. |
| service.port | int | `3000` | The port of the Kubernetes service. |
| serviceAccount.name | string | `""` | If left empty, a service account will be created. |
| storage.adapter.claimName | string | `"adapter-pvc"` |  |
| storage.adapter.createPersistentVolumeClaim | bool | `true` |  |
| storage.adapter.size | string | `"100Gi"` |  |
| storage.adapter.storageClassName | string | `""` |  |
| storage.dataset.claimName | string | `"dataset-pvc"` |  |
| storage.dataset.createPersistentVolumeClaim | bool | `true` |  |
| storage.dataset.size | string | `"100Gi"` |  |
| storage.dataset.storageClassName | string | `""` |  |
| storage.huggingface.claimName | string | `"huggingface-pvc"` |  |
| storage.huggingface.createPersistentVolumeClaim | bool | `true` |  |
| storage.huggingface.size | string | `"100Gi"` |  |
| storage.huggingface.storageClassName | string | `""` |  |
| storage.model.claimName | string | `"model-pvc"` |  |
| storage.model.createPersistentVolumeClaim | bool | `true` |  |
| storage.model.size | string | `"1000Gi"` |  |
| storage.model.storageClassName | string | `""` |  |
| tailscale.apiKey | string | `""` | The Tailscale API key. If headscale.enabled is true, this is ignored. |
| tailscale.authKey | string | `""` | The Tailscale auth key. If headscale.enabled is true, this is ignored. |
| tailscale.imageTag | string | `"stable"` | The Tailscale sidecar image tag. |
| tailscale.loginServer | string | `"https://login.tailscale.com"` | The Tailscale host to connect to. If headscale.enabled is true, this is ignored. |
| tailscale.organization | string | `""` | The name of the Tailscale organization. If headscale.enabled is true, this is ignored. |
| tailscale.resources | object | `{}` | The resource requests and limits for the Nebulous server's Tailscale sidecar container. |
| tailscale.secret.keys.apiKey | string | `"API_KEY"` | The key in the secret containing the Tailscale API key. |
| tailscale.secret.keys.authKey | string | `"AUTH_KEY"` | The key in the secret containing the Tailscale auth key. |
| tailscale.secret.name | string | `"tailscale-secret"` | Name of the secret with the Tailscale auth key and API key. |

