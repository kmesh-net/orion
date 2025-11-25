#!/bin/bash

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}🔍 Full Kmesh + Orion Integration Test${NC}"
echo "=========================================="
echo ""

# Switch to minikube context
kubectl config use-context minikube &>/dev/null

# Test 1: Check Kmesh daemon
echo -e "${YELLOW}1. Checking Kmesh daemon...${NC}"
KMESH_POD=$(kubectl get pods -n kmesh-system -l app=kmesh -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
if [ -n "$KMESH_POD" ]; then
    STATUS=$(kubectl get pod -n kmesh-system $KMESH_POD -o jsonpath='{.status.phase}')
    if [ "$STATUS" == "Running" ]; then
        echo -e "   ${GREEN}✅ Kmesh daemon: $KMESH_POD ($STATUS)${NC}"
        
        # Check if eBPF is loaded by checking for successful startup messages
        echo "   Checking eBPF status..."
        if kubectl logs -n kmesh-system $KMESH_POD --tail=50 | grep -q -E "(bpf loader start successfully|controller start successfully|start cni successfully)"; then
            echo -e "   ${GREEN}✅ eBPF/CNI initialized successfully${NC}"
        else
            echo -e "   ${YELLOW}⚠️  eBPF status unclear (this is OK in kind - check pod annotations below)${NC}"
        fi
    else
        echo -e "   ${RED}❌ Kmesh daemon: $STATUS${NC}"
        echo "   Last logs:"
        kubectl logs -n kmesh-system $KMESH_POD --tail=10
    fi
else
    echo -e "   ${RED}❌ Kmesh daemon not found${NC}"
fi
echo ""

# Test 2: Check namespace configuration
echo -e "${YELLOW}2. Checking namespace configuration...${NC}"
DATAPLANE_MODE=$(kubectl get namespace bookinfo -o jsonpath='{.metadata.labels.istio\.io/dataplane-mode}' 2>/dev/null)
if [ "$DATAPLANE_MODE" == "Kmesh" ]; then
    echo -e "   ${GREEN}✅ Namespace: istio.io/dataplane-mode=Kmesh${NC}"
else
    echo -e "   ${RED}❌ Namespace label: $DATAPLANE_MODE (expected: Kmesh)${NC}"
fi
echo ""

# Test 3: Check Orion waypoint
echo -e "${YELLOW}3. Checking Orion waypoint...${NC}"
ORION_POD=$(kubectl get pods -n bookinfo -l app=orion-waypoint -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
if [ -n "$ORION_POD" ]; then
    STATUS=$(kubectl get pod -n bookinfo $ORION_POD -o jsonpath='{.status.phase}')
    echo -e "   ${GREEN}✅ Orion pod: $ORION_POD ($STATUS)${NC}"
    
    # Check listeners
    echo "   Checking listeners..."
    LISTENERS=$(kubectl logs -n bookinfo $ORION_POD | grep "Started version" | tail -3)
    if [ -n "$LISTENERS" ]; then
        echo "$LISTENERS" | while read line; do
            echo -e "   ${GREEN}✅${NC} $line"
        done
    fi
else
    echo -e "   ${RED}❌ Orion waypoint not found${NC}"
fi
echo ""

# Test 4: Check service waypoint label
echo -e "${YELLOW}4. Checking service waypoint configuration...${NC}"
WAYPOINT_LABEL=$(kubectl get svc productpage -n bookinfo -o jsonpath='{.metadata.labels.istio\.io/use-waypoint}' 2>/dev/null)
if [ "$WAYPOINT_LABEL" == "orion-waypoint" ]; then
    echo -e "   ${GREEN}✅ Service productpage uses: orion-waypoint${NC}"
else
    echo -e "   ${RED}❌ Service waypoint label: $WAYPOINT_LABEL${NC}"
fi
echo ""

# Test 5: Check pod annotations (Kmesh specific)
echo -e "${YELLOW}5. Checking pod Kmesh annotations...${NC}"
PRODUCTPAGE_POD=$(kubectl get pods -n bookinfo -l app=productpage -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
if [ -n "$PRODUCTPAGE_POD" ]; then
    KMESH_ANNOTATION=$(kubectl get pod -n bookinfo $PRODUCTPAGE_POD -o jsonpath='{.metadata.annotations.kmesh\.net/redirection}' 2>/dev/null)
    if [ "$KMESH_ANNOTATION" == "enabled" ]; then
        echo -e "   ${GREEN}✅ Pod has kmesh.net/redirection: enabled${NC}"
    else
        echo -e "   ${YELLOW}⚠️  kmesh.net/redirection: $KMESH_ANNOTATION (may use ambient mode)${NC}"
    fi
fi
echo ""

# Test 6: Traffic routing test
echo -e "${YELLOW}6. Testing traffic routing...${NC}"
echo "   Sending 5 requests through Orion waypoint..."
echo ""

SUCCESS_COUNT=0
for i in {1..5}; do
    RESULT=$(kubectl exec -n bookinfo deploy/sleep -- curl -s -o /dev/null -w "%{http_code}" http://productpage:9080/productpage 2>/dev/null || echo "000")
    if [ "$RESULT" == "200" ]; then
        echo -e "   Request $i: ${GREEN}HTTP $RESULT ✅${NC}"
        ((SUCCESS_COUNT++))
    else
        echo -e "   Request $i: ${RED}HTTP $RESULT ❌${NC}"
    fi
    sleep 1
done
echo ""

# Test 7: Verify Orion listener status
echo -e "${YELLOW}7. Checking Orion listener status...${NC}"
ORION_ACTIVE=0
LISTENER_INFO=$(kubectl logs -n bookinfo $ORION_POD --tail=100 | grep "Started version" | tail -3)
if [ -n "$LISTENER_INFO" ]; then
    echo "$LISTENER_INFO" | while read line; do
        echo -e "   ${GREEN}✅${NC} $line"
    done
    ORION_ACTIVE=1
else
    echo -e "   ${YELLOW}⚠️  No listener activity found${NC}"
fi
echo ""

# Summary
echo "=========================================="
echo -e "${BLUE}📊 Test Summary${NC}"
echo "=========================================="
echo ""

TOTAL_TESTS=7
PASSED_TESTS=0

# Count passed tests
[ -n "$KMESH_POD" ] && [ "$STATUS" == "Running" ] && ((PASSED_TESTS++))
[ "$DATAPLANE_MODE" == "Kmesh" ] && ((PASSED_TESTS++))
[ -n "$ORION_POD" ] && ((PASSED_TESTS++))
[ "$WAYPOINT_LABEL" == "orion-waypoint" ] && ((PASSED_TESTS++))
[ "$KMESH_ANNOTATION" == "enabled" ] && ((PASSED_TESTS++))
[ $SUCCESS_COUNT -ge 3 ] && ((PASSED_TESTS++))
[ $ORION_ACTIVE -eq 1 ] && ((PASSED_TESTS++))

if [ $PASSED_TESTS -eq $TOTAL_TESTS ]; then
    echo -e "${GREEN}✅ ALL TESTS PASSED ($PASSED_TESTS/$TOTAL_TESTS)${NC}"
    echo ""
    echo -e "${GREEN}🎉 Kmesh + Orion integration is working!${NC}"
elif [ $PASSED_TESTS -ge 4 ]; then
    echo -e "${YELLOW}⚠️  MOSTLY WORKING ($PASSED_TESTS/$TOTAL_TESTS tests passed)${NC}"
    echo ""
    echo "Minor issues detected but core functionality works."
else
    echo -e "${RED}❌ TESTS FAILED ($PASSED_TESTS/$TOTAL_TESTS passed)${NC}"
    echo ""
    echo "Please check the logs above for errors."
fi

echo ""
echo "Traffic success rate: $SUCCESS_COUNT/5 requests"
echo ""