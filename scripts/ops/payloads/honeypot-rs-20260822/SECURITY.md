# Security policy

Report vulnerabilities privately through the repository's GitHub Security Advisory flow after publication. Include the affected commit, route, expected behavior, and a minimal reproduction that contains no production secrets or third-party traffic.

Do not test a honeytoken against another system. Every token emitted by this service is synthetic and should fail everywhere.

The following are prohibited without prior written authorization:

- denial-of-service testing;
- attempts to escape the container or access neighboring workloads;
- malware upload or execution;
- harvesting or publishing source addresses or personal information;
- using decoy values against third-party services.
