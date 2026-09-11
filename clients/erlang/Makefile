.PHONY: test clean

BUILD := build

$(BUILD):
	mkdir -p $(BUILD)

$(BUILD)/network_mutex_protocol.beam: src/network_mutex_protocol.erl | $(BUILD)
	erlc -o $(BUILD) src/network_mutex_protocol.erl

test: $(BUILD)/network_mutex_protocol.beam
	escript test_protocol.escript

clean:
	rm -rf $(BUILD)

