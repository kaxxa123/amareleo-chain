// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[cfg(test)]
mod tests {
    use crate::api_node::AmareleoApi;
    use serial_test::serial;

    #[tokio::test]
    #[serial]
    async fn test_node_ledger_path_conflict() {
        let temp_path = std::env::temp_dir();

        // Configure first node on port 3030
        let mut node1 = AmareleoApi::default();
        node1.cfg_ledger(false, Some(temp_path.clone()), false).cfg_rest("127.0.0.1:3030".parse().unwrap(), 10);

        // Start and end the first node
        assert!(node1.start().await.is_ok());
        node1.end().await;

        // Configure second node on port 3031
        let mut node2 = AmareleoApi::default();
        node2.cfg_ledger(false, Some(temp_path.clone()), false).cfg_rest("127.0.0.1:3031".parse().unwrap(), 10);

        // Starting the node fails due to snarkVM restriction described here:
        // https://github.com/kaxxa123/amareleo-chain/issues/19
        assert!(node2.start().await.is_err());
        node2.end().await;
    }

    #[tokio::test]
    #[serial]
    async fn test_node_restart() {
        let temp_path = std::env::temp_dir();

        // Configure node on port 3030
        let mut node1 = AmareleoApi::default();
        node1.cfg_ledger(false, Some(temp_path.clone()), false).cfg_rest("127.0.0.1:3030".parse().unwrap(), 10);

        // Start and end the node
        assert!(node1.start().await.is_ok());
        node1.end().await;

        // Restart the node with the exact same settings...
        let mut node2 = AmareleoApi::default();
        node2.cfg_ledger(false, Some(temp_path.clone()), false).cfg_rest("127.0.0.1:3030".parse().unwrap(), 10);

        // Start and end the node
        assert!(node2.start().await.is_ok());
        node2.end().await;
    }
}
