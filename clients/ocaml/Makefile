.PHONY: test clean

test_protocol: network_mutex_protocol.ml test_protocol.ml
	ocamlc -o test_protocol network_mutex_protocol.ml test_protocol.ml

test: test_protocol
	./test_protocol

clean:
	rm -f *.cmi *.cmo test_protocol

