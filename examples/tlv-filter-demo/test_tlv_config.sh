#!/bin/bash

echo "🧪 TLV Filter Test"
echo "=================="

# Check if client is available
if [ -f "./send_tlv" ]; then
    HAS_CLIENT=true
    echo "✅ TLV client found - will run end-to-end tests"
else
    HAS_CLIENT=false
    echo "⚠️  TLV client not found - will run configuration tests only"
    echo "   To enable full testing, compile the client:"
    echo "   rustc send_tlv.rs -o send_tlv"
fi

# Function to send TLV packet and check response
test_tlv_packet() {
    local ip="$1"
    local port="$2"
    local listener_port="${3:-10000}"

    echo "Testing TLV packet with original destination: $ip:$port"

    # Send TLV packet using Rust binary
    if ./send_tlv "$ip" "$port"; then
        echo "✅ TLV packet sent to listener"
        return 0
    else
        echo "❌ Failed to send TLV packet"
        return 1
    fi
}

echo "Testing TLV filter configuration loading..."

# Start Orion in background
../../target/debug/orion -c orion-config.yaml > orion.log 2>&1 &
ORION_PID=$!

# Wait for Orion to start
sleep 3

if ! kill -0 $ORION_PID 2>/dev/null; then
    echo "❌ Orion failed to start"
    cat orion.log
    exit 1
fi

echo "✅ Orion started (PID: $ORION_PID)"

# Check configuration loading
if grep -q "TLV listener filter is true" orion.log; then
    echo "✅ TLV listener filter loaded and activated"
    TLV_LOADED=true
else
    echo "❌ TLV listener filter not found"
    TLV_LOADED=false
fi

if grep -q "Listener tlv_demo_listener" orion.log; then
    echo "✅ TLV demo listener configuration parsed"
    CONFIG_PARSED=true
else
    echo "❌ TLV demo listener configuration failed"
    CONFIG_PARSED=false
fi

if grep -q "failed to decode TypedStruct" orion.log; then
    echo "❌ TypedStruct parsing failed"
    TYPED_STRUCT_OK=false
else
    echo "✅ TypedStruct configuration processed"
    TYPED_STRUCT_OK=true
fi

echo ""
echo "Configuration Test Results:"
echo "TLV Filter: $([ "$TLV_LOADED" = true ] && echo "✅ PASS" || echo "❌ FAIL")"
echo "Config: $([ "$CONFIG_PARSED" = true ] && echo "✅ PASS" || echo "❌ FAIL")"
echo "TypedStruct: $([ "$TYPED_STRUCT_OK" = true ] && echo "✅ PASS" || echo "❌ FAIL")"

# Test TLV packet processing (only if client is available)
if [ "$HAS_CLIENT" = true ]; then
    echo ""
    echo "Testing TLV packet processing..."

    # Clear previous logs
    > orion.log

    # Send test TLV packet
    test_tlv_packet "192.168.1.100" "8080"

    # Wait a moment for processing
    sleep 1

    # Check if TLV was processed
    if grep -q "Extracted original destination" orion.log; then
        echo "✅ TLV packet processed successfully"
        TLV_PROCESSED=true
    else
        echo "❌ TLV packet processing failed"
        TLV_PROCESSED=false
    fi

    if grep -q "192.168.1.100:8080" orion.log; then
        echo "✅ Original destination extracted correctly"
        ORIGINAL_DEST_EXTRACTED=true
    else
        echo "❌ Original destination extraction failed"
        ORIGINAL_DEST_EXTRACTED=false
    fi
else
    echo ""
    echo "Skipping TLV packet processing test (client not available)"
    TLV_PROCESSED=true  # Not applicable
    ORIGINAL_DEST_EXTRACTED=true  # Not applicable
fi

echo ""
if [ "$HAS_CLIENT" = true ]; then
    echo "TLV Processing Test Results:"
    echo "TLV Processed: $([ "$TLV_PROCESSED" = true ] && echo "✅ PASS" || echo "❌ FAIL")"
    echo "Original Dest: $([ "$ORIGINAL_DEST_EXTRACTED" = true ] && echo "✅ PASS" || echo "❌ FAIL")"
else
    echo "TLV Processing Test Results: ⚠️  SKIPPED (client not available)"
fi

# Cleanup
kill $ORION_PID 2>/dev/null
wait $ORION_PID 2>/dev/null

echo ""
if [ "$HAS_CLIENT" = true ]; then
    # Full end-to-end test
    if [ "$TLV_LOADED" = true ] && [ "$CONFIG_PARSED" = true ] && [ "$TYPED_STRUCT_OK" = true ] && [ "$TLV_PROCESSED" = true ] && [ "$ORIGINAL_DEST_EXTRACTED" = true ]; then
        echo "🎉 TLV Filter End-to-End Test: ALL TESTS PASSED!"
        exit 0
    else
        echo "⚠️  Some tests failed. Check orion.log for details."
        exit 1
    fi
else
    # Configuration-only test
    if [ "$TLV_LOADED" = true ] && [ "$CONFIG_PARSED" = true ] && [ "$TYPED_STRUCT_OK" = true ]; then
        echo "🎉 TLV Filter Configuration Test: PASSED!"
        echo "   Note: End-to-end testing skipped (compile client for full test)"
        exit 0
    else
        echo "⚠️  Configuration tests failed. Check orion.log for details."
        exit 1
    fi
fi
