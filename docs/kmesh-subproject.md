## Background
KMesh is using the Waypoint proxy to handle Level 7 traffic. The Waypoint Proxy is a fork of Istio Proxy, which is a fork of Envoy. While Envoy is a mature application widely used in Kubernetes deployments, we now have a unique opportunity to replace an Istio proxy with something faster, safer and more modern.

## Orion Proxy
Orion Proxy is a proxy application developed at Huawei Ireland Research Lab. We built the Orion Proxy using the Rust programming language to achieve good performance, scalability, memory safety and portability. We wanted to ensure that the Orion proxy fits into the existing Kubernetes ecosystem. We made a pragmatic decision that the Orion Proxy should support the Envoy xDS protocol. This would enable the administrators or operators to dynamically configure the Orion Proxy in the same way as Envoy/Istio proxies are.

## Architecture
The architecture of the Orion Proxy is based on high-quality components and libraries provided by the Rust ecosystem. The Orion Proxy implementation is underpinned by the Tokio runtime.  Tokio enables asynchronous processing of requests resulting in achieving very high throughput and good scalability. Tokio also provides us with well-defined building blocks to build a solution with a well-defined, manageable and extendable architecture. 
We decided to use the Hyper library to handle Http1/Http2 traffic. The Hyper library combined with Rustls allowed us to handle modern TLS traffic through a secure and FIPS-compliant solution.

## Performance
Before release, we rigorously tested Orion's performance in comparison to Envoy Proxy. The [results](./performance/performance.md) show that the Orion Proxy outperforms the Envoy Proxy in terms of throughput and request latency.

## Features and Future Roadmap
Most of our efforts up to date, have been focused on ensuring that the Orion Proxy can reliably proxy Level 4 and Level 7 traffic. In future, we plan to shift our objectives and implement more features that will make Orion Proxy more robust, reliable and straightforward to operate in production environments. In the short term, we want to provide features such as access logging, metrics with open telemetry, and support for HAProxy and Websockets protocols.

## Current and Past Contributors

### Current 
|Name| Affiliation|  Contact |
|--------|------|---|
|Alan Keane| Huawei Ireland Research Lab|alan.keane1@huawei.com|
|Dawid Nowak| Huawei Ireland Research Lab|dawid.nowak@huawei.com|
|Francesco Ciaccia|  Huawei Ireland Research Lab|francesco.ciaccia1@huawei-partners.com|
|Nicola Bonelli |  Huawei Ireland Research Lab|nicola.bonelli@huawei-partners.com|
|Wang Ruize | Huawei| wangruize1@huawei.com|


### Past
|Name| Affiliation|
|--------|------|
|Liu Xiang | Huawei| 
|Rui Ferreira |  Huawei Ireland Research Lab|
|Oriol Arcas | Huawei Ireland Research Lab| 
|Hayley Deckers | Huawei Ireland Research Lab| 



## License
Apache License, Version 2.0

## Project Link
[Orion Proxy](https://gitee.com/orion-proxy/orion)







