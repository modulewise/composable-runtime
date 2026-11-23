# the cli, released with an archive for each platform, and published to crates.io
CLI_BIN := composable
PUBLISH_CRATES := composable-interceptor composable-runtime composable-http-gateway

# the cli installed as a tool built from the workspace, like a cli the components are built with, e.g.
# the constants cli in componentized/constants, run as `$(COMPONENTIZED_BUILD)`
LOCAL_TOOLS := composable-runtime composable-http-server

# exclude http-client from publishing in addition to the typical patterns
UNPUBLISHED_COMPONENTS := "dep-% %-dep internal-% %-internal test-% %-test http-client"


include build/build.mk

test: check-components validate-components
test-integration: test-cli test-examples


# the cli installed with the tools, run in this repository as the CI workflow runs the cli unpacked
# from its archive, see scripts/smoke-test-cli.sh
.PHONY: test-cli ## Run the componentized-build cli installed with the tools
test-cli: $(call tool,composable-runtime)
	scripts/smoke-test-cli.sh $(COMPOSABLE)

.PHONY: test-examples
test-examples:
	@true

.PHONY: clean-examples
clean-all: clean-examples
clean-examples:
	@true

.PHONY: test-example-7cs
test-examples: test-example-7cs 
test-example-7cs: examples/7cs/wit/deps $(call tool,wasm-tools) $(call tool,composable-runtime)
	( cd examples/7cs && ./build.sh )
	( cd examples/7cs && ./run.sh 1 )

.PHONY: clean-example-7cs
clean-examples: clean-example-7cs 
clean-example-7cs:
# 	rm -rf examples/7cs/wit/deps
	rm -rf examples/7cs/lib/*.wasm
	( cd examples/7cs/components && cargo clean )

.PHONY: test-example-filesystem
test-examples: test-example-filesystem
test-example-filesystem: examples/filesystem/wit/deps $(call tool,wasm-tools) $(call tool,composable-runtime)
	( cd examples/filesystem && ./build.sh )
	( cd examples/filesystem && ./run.sh )

.PHONY: clean-example-filesystem
clean-examples: clean-example-filesystem 
clean-example-filesystem:
# 	rm -rf examples/filesystem/wit/deps
	rm -rf examples/filesystem/lib/*.wasm
	rm -f examples/filesystem/data/from-guest.txt
	( cd examples/filesystem/file-store && cargo clean )

.PHONY: test-example-function
test-examples: test-example-function
test-example-function: $(call tool,composable-runtime)
	( cd examples/function && ./run.sh )

.PHONY: clean-example-function
clean-examples: clean-example-function 
clean-example-function:
	@true

.PHONY: test-example-function-adapter
test-examples: test-example-function-adapter
test-example-function-adapter: $(call tool,wkg) $(call tool,wasm-tools) $(call tool,composable-runtime)
	( cd examples/function-adapter && ./build.sh )
	( cd examples/function-adapter && ./run.sh )

.PHONY: clean-example-function-adapter
clean-examples: clean-example-function-adapter 
clean-example-function-adapter:
	rm -rf examples/function-adapter/lib/*.wasm

.PHONY: test-example-hello-world
test-examples: test-example-hello-world
test-example-hello-world: examples/hello-world/wit/deps $(call tool,wasm-tools) $(call tool,composable-runtime)
	( cd examples/hello-world && ./build.sh )
	( cd examples/hello-world && ./run.sh )

.PHONY: clean-example-hello-world
clean-examples: clean-example-hello-world 
clean-example-hello-world:
# 	rm -rf examples/hello-world/wit/deps
	rm -rf examples/hello-world/lib/*.wasm
	( cd examples/hello-world/greeter && cargo clean )

.PHONY: test-example-host-capability
test-examples: test-example-host-capability
test-example-host-capability: $(call tool,wasm-tools)
	( cd examples/host-capability && ./build.sh )
	( cd examples/host-capability && ./run.sh )

.PHONY: clean-example-host-capability
clean-examples: clean-example-host-capability 
clean-example-host-capability:
# 	rm -rf examples/host-capability/wit/deps
	rm -rf examples/host-capability/lib/*.wasm
	( cd examples/host-capability && cargo clean )

.PHONY: test-example-http-server
test-examples: test-example-http-server
test-example-http-server: $(call tool,wasm-tools) $(call tool,composable-http-server)
	( cd examples/http-server && exec ./run-server.sh ) & pid=$$!; \
	trap 'pkill -P $$pid 2>/dev/null; kill $$pid 2>/dev/null; wait $$pid 2>/dev/null' EXIT; \
	for i in $$(seq 1 300); do \
		curl -s -o /dev/null localhost:8888 && break; \
		kill -0 $$pid 2>/dev/null || { echo "http-server exited before listening"; exit 1; }; \
		sleep 1; \
	done; \
	curl -s -o /dev/null localhost:8888 || { echo "timed out waiting for port 8888"; exit 1; }; \
	( cd examples/http-server && ./greet.sh )

.PHONY: clean-example-http-server
clean-examples: clean-example-http-server 
clean-example-http-server:
	@true

.PHONY: test-example-interceptor
test-examples: test-example-interceptor
test-example-interceptor: examples/interceptor/wit/deps $(call tool,wasm-tools) $(call tool,composable-runtime)
	( cd examples/interceptor && ./build.sh )
	( cd examples/interceptor && ./run.sh )

.PHONY: clean-example-interceptor
clean-examples: clean-example-interceptor 
clean-example-interceptor:
# 	rm -rf examples/interceptor/wit/deps
	rm -rf examples/interceptor/lib/*.wasm
	( cd examples/interceptor && cargo clean )

.PHONY: test-example-local-registry
test-examples: test-example-local-registry
test-example-local-registry: test-example-hello-world $(call tool,wkg) $(call tool,composable-runtime)
	( cd examples/local-registry && ./start-registry.sh )
	( cd examples/local-registry && ./push-component.sh )
	( cd examples/local-registry && ./run.sh )
	( cd examples/local-registry && ./list.sh )
	( cd examples/local-registry && ./stop-registry.sh )

.PHONY: clean-example-local-registry
clean-examples: clean-example-local-registry 
clean-example-local-registry:
	@true

.PHONY: test-example-messaging
test-examples: test-example-messaging
test-example-messaging: test-example-hello-world $(call tool,composable-runtime)
	( cd examples/messaging && ./publish.sh Alice)
	( cd examples/messaging && ./publish.sh Bob)

.PHONY: clean-example-messaging
clean-examples: clean-example-messaging 
clean-example-messaging:
	@true

.PHONY: test-example-named-imports
test-examples: test-example-named-imports
test-example-named-imports: examples/named-imports/wit/deps $(call tool,wasm-tools) $(call tool,composable-runtime)
	( cd examples/named-imports && ./build.sh)
	( cd examples/named-imports && ./run.sh)

.PHONY: clean-example-named-imports
clean-examples: clean-example-named-imports 
clean-example-named-imports:
# 	rm -rf examples/named-imports/wit/deps
	rm -rf examples/named-imports/lib/*.wasm
	( cd examples/named-imports && cargo clean )

.PHONY: test-example-otel-components
test-examples: test-example-otel-components
test-example-otel-components: examples/otel-components/wit/deps $(call tool,wasm-tools) $(call tool,composable-runtime)
	( cd examples/otel-components && ./build.sh)
	( cd examples/otel-components && ./run.sh)

.PHONY: clean-example-otel-components
clean-examples: clean-example-otel-components 
clean-example-otel-components:
# 	rm -rf examples/otel-components/wit/deps
	rm -rf examples/otel-components/lib/*.wasm
	( cd examples/otel-components && cargo clean )

.PHONY: test-example-otel-service
test-examples: test-example-otel-service
test-example-otel-service: examples/otel-service/wit/deps $(call tool,wasm-tools) $(call tool,composable-runtime)
	( cd examples/otel-service && ./build.sh)
	( cd examples/otel-service && exec ./run.sh ) & pid=$$!; \
	trap 'pkill -P $$pid 2>/dev/null; kill $$pid 2>/dev/null; wait $$pid 2>/dev/null' EXIT; \
	for i in $$(seq 1 300); do \
		curl -s -o /dev/null localhost:8080 && break; \
		kill -0 $$pid 2>/dev/null || { echo "http-server exited before listening"; exit 1; }; \
		sleep 1; \
	done; \
	curl -s -o /dev/null localhost:8080 || { echo "timed out waiting for port 8080"; exit 1; }; \
	( cd examples/otel-service && ./curl.sh)

.PHONY: clean-example-otel-service
clean-examples: clean-example-otel-service 
clean-example-otel-service:
# 	rm -rf examples/otel-service/wit/deps
	rm -rf examples/otel-service/lib/*.wasm
	( cd examples/otel-service && cargo clean )

.PHONY: test-example-service
test-examples: test-example-service
test-example-service: $(call tool,wasm-tools)
	( cd examples/service && ./build.sh)
	( cd examples/service && ./run.sh)

.PHONY: clean-example-service
clean-examples: clean-example-service 
clean-example-service:
# 	rm -rf examples/service/wit/deps
	rm -rf examples/service/lib/*.wasm
	( cd examples/service && cargo clean )
