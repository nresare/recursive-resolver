Just a place to jot down little things:

* We currently depend on the connection like nature of UdpSocket, not checking query ID
* The cache grabs and releases its lock a good many times per req/resp cycle
* Lets get rid of IO errors and serde errors from ResolutionError
* We are always using the first IP returned. We need a new abstraction here
* 