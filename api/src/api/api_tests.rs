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
    use crate::{AmareleoApiState, api_node::AmareleoApi};
    use amareleo_chain_tracing::TracingHandler;
    use serial_test::serial;
    use snarkvm::prelude::{MainnetV0, Network};

    #[test]
    #[serial]
    fn test_singleton_reference_identity() {
        let instance1 = AmareleoApi::new();
        let instance2 = AmareleoApi::new();

        // Check that both references point to the same memory address
        assert!(std::ptr::eq(instance1, instance2), "Singleton returned different references!");
    }

    #[tokio::test]
    #[serial]
    async fn test_node_state() {
        let temp_path = std::env::temp_dir();

        // Test setters
        // Configure first node on port 3030
        let node1 = AmareleoApi::new();
        assert!(node1.cfg_network(MainnetV0::ID).is_ok());
        assert!(node1.cfg_ledger(false, Some(temp_path.clone()), true).is_ok());
        assert!(node1.cfg_rest("127.0.0.1:3030".parse().unwrap(), 10).is_ok());
        assert!(node1.cfg_file_log(None, 4).is_ok());
        assert!(node1.cfg_custom_log(TracingHandler::new()).is_ok());
        assert!(node1.cfg_no_log().is_ok());
        //======================================

        // Test getters
        assert!(!node1.is_started());
        assert!(node1.get_log_file().is_err());

        // Well known ledger path when:
        // - base path is set manually
        // - default naming enabled
        // - MainnetV0
        let ledger = node1.get_ledger_folder();
        assert!(ledger.is_ok());

        let ledger = ledger.unwrap();
        let mut expected = temp_path.clone();
        expected.push(".amareleo-tmp-ledger-0-0");
        assert_eq!(ledger, expected);
        //======================================

        // Start node
        assert!(node1.get_state() == AmareleoApiState::Init);
        assert!(node1.start().await.is_ok());
        //======================================

        // Test setters
        // Once started config is locked completely
        assert!(node1.cfg_network(MainnetV0::ID).is_err());
        assert!(node1.cfg_ledger(false, Some(temp_path.clone()), true).is_err());
        assert!(node1.cfg_rest("127.0.0.1:3031".parse().unwrap(), 10).is_err());
        assert!(node1.cfg_file_log(None, 4).is_err());
        assert!(node1.cfg_custom_log(TracingHandler::new()).is_err());
        assert!(node1.cfg_no_log().is_err());
        //======================================

        // Test getters
        assert!(node1.is_started());
        //======================================

        // Stop node
        assert!(node1.get_state() == AmareleoApiState::Started);
        assert!(node1.end().await.is_ok());
        //======================================

        // Calling new will return the same instance.
        // We call this as another confirmation that we are working with a singleton.
        let node2 = AmareleoApi::new();

        // Test setters
        // Some config properties cannot ever change after
        // the first start even if stopped...
        assert!(node2.cfg_network(MainnetV0::ID).is_err());
        assert!(node2.cfg_ledger(false, Some(temp_path.clone()), false).is_err());
        assert!(node2.cfg_rest("127.0.0.1:3031".parse().unwrap(), 10).is_err());

        // Some config properties can still change
        assert!(node2.cfg_custom_log(TracingHandler::new()).is_ok());
        assert!(node2.cfg_no_log().is_ok());
        assert!(node2.cfg_file_log(None, 4).is_ok());
        //======================================

        // Test getters
        assert!(!node2.is_started());

        // Well known default log file path when:
        // - NOT keeping staate,
        // - MainnetV0
        // - REST 127.0.0.1:3030
        let logfile = node2.get_log_file();
        assert!(logfile.is_ok());

        let logfile = logfile.unwrap();
        let mut expected = temp_path.clone();
        expected.push("amareleo-chain-tmp-0-8e4fb233.log");
        assert_eq!(logfile, expected);

        // Well known ledger path when:
        // - base path is set manually
        // - default naming enabled
        // - MainnetV0
        let ledger = node2.get_ledger_folder();
        assert!(ledger.is_ok());

        let ledger = ledger.unwrap();
        let mut expected = temp_path.clone();
        expected.push(".amareleo-tmp-ledger-0-0");
        assert_eq!(ledger, expected);
        //======================================

        // Re-start the node
        assert!(node2.get_state() == AmareleoApiState::Stopped);
        assert!(node2.start().await.is_ok());

        assert!(node2.get_state() == AmareleoApiState::Started);
        assert!(node2.end().await.is_ok());

        assert!(node2.get_state() == AmareleoApiState::Stopped);
    }
}
