This is a tiny experiment attempting to build a fully recursive
DNS resolver based on the primitives that hickory-dns provides.

If it works out, I expect to contribute this implementation to 
the hickory-dns project.

# Current status

The basic functionality of doing recursive name resolution works in fair weather. There is 
a cache so performance should not be terrible. There is still quite a todo list:

- [ ] Implement timeouts and resends
- [ ] IPv6 support
- [ ] Handling of truncated responses with retry over TCP transport
- [ ] Some smartness selecting which NS to use for some zone, keeping track of health and performance
- [ ] Support for listening on multiple interfaces not just a wildcard one
- [ ] Responding to queries over TCP
- [ ] DNSSec

# License

As with hickory, this is dual-licensed with Apache and MIT licensees
