use std::convert::TryInto;
use tonic::codegen::StdError;
use tonic::transport::Endpoint;
use std::collections::HashMap;

pub type EtcdResult<T> = Result<T, Box<dyn std::error::Error>>;

// Internal names, which are unfortunately named.
pub mod mvccpb {
    // Proto file: kv.proto
    tonic::include_proto!("mvccpb");
}

pub mod authpb {
    // Proto file: auth.proto
    tonic::include_proto!("authpb");
}

pub mod etcdserver {
    // Proto file: rpc.proto
    tonic::include_proto!("etcdserverpb");
}

use etcdserver::client;

/// Range of keys
pub struct Range<'a, 'b, T> {
    start: &'b str,
    end: Option<&'b str>,
    client: &'a mut EtcdClient<T>,
}

impl<'a, 'b> Range<'a, 'b, tonic::transport::channel::Channel> {
    pub async fn put<S>(&mut self, value: S) -> EtcdResult<()>
    where S: Into<String>
    {
        let request = etcdserver::PutRequest {
            key: self.start.to_string().into_bytes(),
            value: value.into().into_bytes(),
            prev_kv: true,
            ..Default::default()
        };

        let _response = self.client.kv_client.put(request).await?;
        // TODO: check the response for errors
        Ok(())
    }

    pub async fn get(&mut self) -> EtcdResult<HashMap<String, String>> {
        let request = etcdserver::RangeRequest {
            key: self.start.to_string().into_bytes(),
            range_end: match self.end {
                Some(s) => s.to_string().into_bytes(),
                None => "".to_string().into_bytes(),
            },
            ..Default::default()
        };
        let response = self.client.kv_client.range(request).await?;
        let range_response = response.into_inner();

        let mut out = HashMap::new();
        range_response.kvs.iter().for_each(|kv| {
            let key = std::str::from_utf8(&kv.key).unwrap();
            let value = std::str::from_utf8(&kv.value).unwrap();

            out.insert(key.to_string(), value.to_string());
        });

        Ok(out)
    }

    pub async fn delete(self) -> EtcdResult<()> {
        let request = etcdserver::DeleteRangeRequest {
            key: self.start.to_string().into_bytes(),
            range_end: match self.end {
                Some(s) => s.to_string().into_bytes(),
                None => "".to_string().into_bytes(),
            },
            ..Default::default()
        };

        let _response = self.client.kv_client.delete_range(request).await?;
        Ok(())
    }
}

/// Cluster information
pub struct Cluster<'a, T> {
    client: &'a mut EtcdClient<T>,
}

impl<'a> Cluster<'a, tonic::transport::channel::Channel> {
    pub async fn members(&mut self) -> EtcdResult<Vec<etcdserver::Member>> {
        let request = etcdserver::MemberListRequest {};
        let response = self.client.cluster_client.member_list(request).await?;
        Ok(response.into_inner().members)
    }
}

/// Etcd client
pub struct EtcdClient<T> {
    #[allow(dead_code)]
    auth_client: client::AuthClient<T>,
    cluster_client: client::ClusterClient<T>,
    kv_client: client::KvClient<T>,
    #[allow(dead_code)]
    lease_client: client::LeaseClient<T>,
    #[allow(dead_code)]
    status_client: client::MaintenanceClient<T>,
    watch_client: client::WatchClient<T>,
}

impl EtcdClient<tonic::transport::channel::Channel> {
    pub async fn connect<D>(dst: D) -> EtcdResult<Self>
    where
        D: TryInto<Endpoint> + Clone,
        D::Error: Into<StdError>,
    {
        let auth_client = client::AuthClient::connect(dst.clone()).await?;
        let cluster_client = client::ClusterClient::connect(dst.clone()).await?;
        let kv_client = client::KvClient::connect(dst.clone()).await?;
        let lease_client = client::LeaseClient::connect(dst.clone()).await?;
        let status_client = client::MaintenanceClient::connect(dst.clone()).await?;
        let watch_client = client::WatchClient::connect(dst.clone()).await?;

        Ok(Self {
            auth_client,
            cluster_client,
            kv_client,
            lease_client,
            status_client,
            watch_client,
        })
    }

    pub fn range<'a, 'b>(
        &'a mut self,
        start: &'b str,
        end: Option<&'b str>,
    ) -> Range<'a, 'b, tonic::transport::channel::Channel> {
        Range {
            start,
            end,
            client: self,
        }
    }

    pub async fn watch<K>(
        &mut self,
        key: K,
    ) -> EtcdResult<tonic::Streaming<etcdserver::WatchResponse>>
    where
        K: Into<Vec<u8>> + Sync + Send + 'static,
    {
        let request = async_stream::stream! {
            let watch_create_req = etcdserver::WatchCreateRequest {
                key: key.into(),
                ..Default::default()
            };
            let request_union = etcdserver::watch_request::RequestUnion::CreateRequest(watch_create_req);
            let request = etcdserver::WatchRequest {
                request_union: Some(request_union),
            };

            yield request;
        };

        let response = self.watch_client.watch(request).await?;
        let inbound = response.into_inner();

        Ok(inbound)
    }

    /*
    pub async fn status(&mut self) -> EtcdResult<etcdserver::StatusResponse> {
        let request = etcdserver::StatusRequest {};
        let response = self.status_client.status(request).await?;
        Ok(response.into_inner())
    }

    pub async fn server_alarms(&mut self) -> EtcdResult<etcdserver::AlarmResponse> {
        let mut request = etcdserver::AlarmRequest::default();
        request.set_action(etcdserver::alarm_request::AlarmAction::Get);
        let response = self.status_client.alarm(request).await?;
        Ok(response.into_inner())
    }

    pub async fn cluster_members(&mut self) -> EtcdResult<etcdserver::MemberListResponse> {
        let request = etcdserver::MemberListRequest {};
        let response = self.cluster_client.member_list(request).await?;
        Ok(response.into_inner())
    }
    */

    pub fn cluster<'a>(&'a mut self) -> Cluster<'a, tonic::transport::channel::Channel> {
        Cluster {
            client: self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connecting_to_instance() {
        let _client = EtcdClient::connect("http://127.0.0.1:2379").await.unwrap();
    }

    #[tokio::test]
    async fn test_ranges() {
        let mut client = EtcdClient::connect("http://127.0.0.1:2379").await.unwrap();
        let mut range = client.range("foo", None);

        range.put("bar").await.unwrap();

        let keys = range.get().await.unwrap();

        assert_eq!(keys["foo"], "bar");

        // Test delete a range
        range.delete().await.unwrap();

        // Have to get the range again as `delete` drops the range.
        let mut range = client.range("foo", None);
        let keys = range.get().await.unwrap();
        assert!(keys.is_empty());
    }

    #[tokio::test]
    async fn test_watching() {
        use futures::channel::oneshot;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let mut client = EtcdClient::connect("http://127.0.0.1:2379").await.unwrap();

        // Get a stream of events from etcd for the "foo" key
        let mut stream = client.watch("foo").await.unwrap();

        // Channel coordinates the watch task, to wait until the value has been seen, then end the
        // task. This ensures we do not need sleeps in the test, but that the two separate spawned
        // tasks can run concurrently.
        let (tx, mut rx) = oneshot::channel();

        // Have we seen a change yet?
        let seen = Arc::new(AtomicBool::new(false));
        let cp = seen.clone();

        // Spawn a task to poll any changes
        tokio::spawn(async move {
            while let Some(_val) = stream.message().await.unwrap() {
                cp.store(true, Ordering::SeqCst);
                tx.send(true).unwrap();
                break;
            }
        });

        // Get a range so we can change the "foo" node value
        let mut range = client.range("foo", None);
        range.put("bar").await.unwrap();

        // Wait until the task finishes
        rx.try_recv().unwrap();

        // Check that we've seen the change
        assert!(seen.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_listing_members() {
        // There is only one member in the test cluster, so we check this.
        let mut client = EtcdClient::connect("http://127.0.0.1:2379").await.unwrap();

        let mut cluster_info = client.cluster();
        let members = cluster_info.members().await.unwrap();
        assert_eq!(members.len(), 1);
    }
}
