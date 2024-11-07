use std::collections::{hash_map, HashMap};

use hyper::StatusCode;

use crate::{Client, client::BoxClient};

pub mod tls;
pub mod trace;

/// A map containing different types of clients.
#[derive(Default)]
pub struct ClientMap(HashMap<String, BoxClient>);

impl ClientMap {
    /// Creates an empty `ClientMap`.
    #[inline]
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Inserts a client into the map.
    #[inline]
    pub fn insert<V: Client>(&mut self, key: String, value: V) {
        self.0.insert(key, Box::new(value));
    }

    /// Returns a reference to the client trait object.
    #[inline]
    pub fn get(&self, key: &str) -> Option<&dyn Client> {
        self.0.get(key).map(|v| v.as_ref())
    }

    /// Returns a mutable reference to the client trait object.
    #[inline]
    pub fn get_mut(&mut self, key: &str) -> Option<&mut dyn Client> {
        self.0.get_mut(key).map(|v| v.as_mut())
    }

    /// Downcasts and returns a reference to the concrete client type.
    #[inline]
    pub fn get_as<V: Client>(&self, key: &str) -> Option<&V> {
        self.0.get(key)?.downcast_ref::<V>()
    }

    /// Downcasts and returns a mutable reference to the concrete client type.
    #[inline]
    pub fn get_mut_as<V: Client>(&mut self, key: &str) -> Option<&mut V> {
        self.0.get_mut(key)?.downcast_mut::<V>()
    }

    /// Removes a client from the map.
    #[inline]
    pub fn remove(&mut self, key: &str) -> Option<Box<dyn Client>> {
        self.0.remove(key)
    }

    /// An iterator visiting all key-value pairs in arbitrary order.
    #[inline]
    pub fn iter(&self) -> hash_map::Iter<'_, String, Box<dyn Client>> {
        self.0.iter()
    }

    /// An iterator visiting all keys in arbitrary order.
    #[inline]
    pub fn keys(&self) -> hash_map::Keys<'_, String, Box<dyn Client>> {
        self.0.keys()
    }

    /// An iterator visiting all values in arbitrary order.
    #[inline]
    pub fn values(&self) -> hash_map::Values<'_, String, Box<dyn Client>> {
        self.0.values()
    }

    /// Returns the number of elements in the map.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the map contains no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Returns equivalent http status code for grpc status code
pub fn grpc_to_http_code(value: tonic::Code) -> StatusCode {
    use tonic::Code::*;
    match value {
        InvalidArgument => StatusCode::BAD_REQUEST,
        Internal => StatusCode::INTERNAL_SERVER_ERROR,
        NotFound => StatusCode::NOT_FOUND,
        DeadlineExceeded => StatusCode::REQUEST_TIMEOUT,
        Unimplemented => StatusCode::NOT_IMPLEMENTED,
        Unauthenticated => StatusCode::UNAUTHORIZED,
        PermissionDenied => StatusCode::FORBIDDEN,
        Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Ok => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}