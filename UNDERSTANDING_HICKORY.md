An attempt to follow the code that resolves dns queries from the recurse binary in util

* The main function parses the nameservers option to SockAddr objects
* Those are then used to build NameServerConfig structs that is pushed into a NameServerConfigGroup
* That object is passed to Recursor::builder().build() which returns a Recursor instance, presumably backed by the 
  passed NameServerConfigGroup
* Resolution happens by calling resolver.resolve().await
* resolve() checks self.record_cache, if it contains the value it returns it
* if looking for NS or DS, set zone to base_name(), else query.name()
* Loops up to 20 times awaiting self.ns_pool_for_zone(): Result<RecursorPool<TokioRuntimeProvider>> passing zone and 
  request_time
* ns_pool_for_zone() calls itself recursively for the parent zone up until the root zone for which the roots member 
  is used 
* Once there is a nameserver_pool, use this to lookup NS of the current zone using the nameserver_pool for it's 
  basename by calling self.lookup()
* Recursor.lookup() takes a query and a RecursorPool<TokioRuntimeProvider> ns and returns a Result<Lookup, Error>
* Recursor.lookup() calls the RecursorPool lookup() and awaits it's response, updating its cache with the responses 
  and returns 
* RecursorPoolholds a zone, a GenericNameServerPool<RuntimeProvider> and an Arc<Mutex<ActiveRequests>>
* RecursorPool.lookup() finds a SharedLookup object in its list if it exists and awaits it, returning the response.
* The SharedLookup is built using the result of GenericNameServerPool.lookup() that is then turned into a future with .into_future()  with an attached map() that remaps the error to the right type wraps it in a box that .shared() is called on

* The GenericNameServerPool.lookup() method returns a Stream<Item = Result<DnsResponse, Err> and is implemented in the DnsHandle trait by building a message, then calling DnsHandle.send()

* The GenericNameServerPool<P> is a type alias to NameServerPool<GenericConnector<P>. NameServerPool has an implementation of DnsHandle
* * Sidebar: GenericConnector implements ConnectionProvider, a trait that has new_conenction() which returns a FutureConn, a Future<DnsHandle>

* NameserverPool has an Arc<[NameServer<P>]> for datagram and one for stream, plus options.

* NameServerPool.send() will take something that can be turned into a DnsRequest and will use Self::try_send() with datagram_conns (ignoring the TCP variant for now.

* NameServerPool.try_send() will then sort the conns and await parallel_conn_loop(conns, request, opts)

* parallel_conn_loop returns Result<DnsResponse> and is implemnted roughly by finding a NameServer<P> and calling its send() with the DnsRequest.

* Send in NameServer implements send() that returns a Pin<Box<Stream<Item = Result<DnsResponse>>>> which in turn calls NameServer.inner_send()

* NameServer.inner_send() calls .connected_mut_client().await? and then await client.send(request) which will hold the result

* connected_mut_client() clones out a client from self.client

* self.client is an Arc<Mutex<Option<P::Conn>, P is ConnectionProvider and its associated type Conn is a DnsHandle



This email and any attachments should not be construed as an offer or recommendation to sell or buy or a solicitation of an offer to sell or buy any specific security, fund or instrument or to participate in any particular investment strategy. The information contained herein is given as of a certain date and does not purport to give information as of any other date. Although the information presented herein has been obtained from sources we believe to be reliable, no representation or warranty, expressed or implied, is made as to the accuracy or completeness of that information. Past performance is not indicative of future results.

CONFIDENTIALITY NOTICE: This message and any attachment are confidential. If you are not the intended recipient, please telephone or email the sender and delete this message and any attachment from your system. If you are not the intended recipient you must not copy this message or attachment or disclose the contents to any other persons.

Balyasny Asset Management (UK) LLP is authorised and regulated by the Financial Conduct Authority in the UK. Balyasny Asset Management LP is registered as an Investment Advisor with the Securities and Exchange Commission in the USA.

BAM prohibits all personnel from having any business related communications over text message or other unapproved communication applications. Unless pre-approved, BAM employees are only permitted to communicate over email, Bloomberg and BAM telephone lines.