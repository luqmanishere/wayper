# Some technical notes

## Offloading rendering

A render queue is created to offload the rendering. A struct holds the job data
and the locks required. A thread is spawned to do the actual rendering. Job details
are sent to the server, and can later be queried with the same key to get the result.
An unfinished request will block the caller's thread, and will start a new render job
if it does not exist.

Data in the struct is `Arc`'d, so a created server instance can simple be cloned
anywhere. Methods also do not take mutable references, so there is no need to put
the server behind a `Mutex`. Instead, the server manages its own locks and
references.

Hopefully, this design can be adapted to render videos when it comes to it...
