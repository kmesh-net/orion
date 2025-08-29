### Simple Level 7 Loadbalancing Demo

0. Install Rust, Docker, Curl etc.

1. Start Orion  

    Start Orion on port 8000 with different routes configured and two different clusters.

```shell
cargo run --bin orion -- --config orion-proxy/conf/orion-runtime-http.yaml
```
2. Start Echo servers as endpoints
```shell
docker run --rm -p 4001:80 ealen/echo-server
docker run --rm -p 4002:80 ealen/echo-server
docker run --rm -p 4003:80 ealen/echo-server
docker run --rm -p 4004:80 ealen/echo-server
```

3. Make some calls with Curl

    Return pre-canned response

```shell
curl -i http://127.0.0.1:8000/direct-response
```

        Route to an endpoint configured in cluster 1. If there are no endpoints available it will return 502

```shell
curl -i http://127.0.0.1:8000/blah
```


