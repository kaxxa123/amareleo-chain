#!/bin/bash

# Set parameters directly
# The first two parameters are unused in amareleo.
# We keep them to facilitate integrating any future
# snarkos updates wihtout risking to break anything.
total_validators=$1
total_clients=$2
network_id=$3
min_height=$4

# Default values if not provided
: "${total_validators:=4}"
: "${total_clients:=2}"
: "${network_id:=0}"
: "${min_height:=45}"

# Determine network name based on network_id
case $network_id in
    0)
        network_name="mainnet"
        ;;
    1)
        network_name="testnet"
        ;;
    2)
        network_name="canary"
        ;;
    *)
        echo "Unknown network ID: $network_id, defaulting to mainnet"
        network_name="mainnet"
        ;;
esac

echo "Using network: $network_name (ID: $network_id)"

# Create log directory
log_dir=".logs-$(date +"%Y%m%d%H%M%S")"
mkdir -p "$log_dir"

# Array to store PIDs of all processes
declare -a PIDS

# Start all validator nodes in the background
log_file="$log_dir/validator-0.log"
amareleo-chain start --network $network_id --logfile $log_file --metrics &
PIDS[0]=$!
echo "Started validator 0 with PID ${PIDS[0]}"

# Function to check block heights
check_heights() {
    echo "Checking block heights on all nodes..."
    all_reached=true
    highest_height=0

    port=$((3030))
    height=$(curl -s "http://127.0.0.1:$port/$network_name/block/height/latest" || echo "0")
    echo "Node 0 block height: $height"
    
    # Track highest height for reporting
    if [[ "$height" =~ ^[0-9]+$ ]] && [ $height -gt $highest_height ]; then
        highest_height=$height
    fi
    
    if ! [[ "$height" =~ ^[0-9]+$ ]] || [ $height -lt $min_height ]; then
        all_reached=false
    fi
    
    if $all_reached; then
        echo "✅ SUCCESS: All nodes reached minimum height of $min_height"
        return 0
    else
        echo "⏳ WAITING: Not all nodes reached minimum height of $min_height (highest so far: $highest_height)"
        return 1
    fi
}

# Wait for 60 seconds to let the network start up
echo "Waiting 60 seconds for network to start up..."
sleep 60

# Check heights periodically with a timeout
total_wait=0
while [ $total_wait -lt 900 ]; do  # 15 minutes max
    if check_heights; then
        echo "🎉 Test passed! All nodes reached minimum height."
        
        # Cleanup: kill all processes
        for pid in "${PIDS[@]}"; do
            kill -9 $pid 2>/dev/null || true
        done
        
        exit 0
    fi
    
    # Continue waiting
    sleep 60
    total_wait=$((total_wait + 60))
    echo "Waited $total_wait seconds so far..."
done

echo "❌ Test failed! Not all nodes reached minimum height within 15 minutes."

# Print logs for debugging
echo "Last 20 lines of validator logs:"
echo "=== Validator 0 logs ==="
tail -n 20 "$log_dir/validator-0.log"

# Cleanup: kill all processes
for pid in "${PIDS[@]}"; do
    kill -9 $pid 2>/dev/null || true
done

exit 1