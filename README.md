This is a tiny experiment attempting to build a fully recursive
DNS resolver based on the primitives that hickory-dns provides.

If it works out, I expect to contribute this implementation to 
the hickory-dns project.

# Todo

- [x] Improve the FakeBackend implementation such that a full successful lookup can be tested
- [x] Handle cross-referencing delegations
- [ ] Figure out how to have target server appear in traces 
- [ ] Implement timeouts
- [ ] Implement caching
- [ ] Implement running in server mode, accepting incoming requests

# License

As with hickory, this is dual-licensed with Apache and MIT licensees
