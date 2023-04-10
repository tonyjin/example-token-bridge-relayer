#/bin/bash

pgrep -f sui-test-validator > /dev/null
if [ $? -eq 0 ]; then
    echo "sui-test-validator already running"
    exit 1;
fi

DEPENDENCIES_DIR=$(pwd)/dependencies
TEST_DIR=$(dirname $0)/../ts/tests
SUI_CONFIG=$TEST_DIR/sui_config

### Remove databases generated by localnet
rm -rf $SUI_CONFIG/*_db

### Start local node
echo "Starting local validator."
sui start \
    --network.config $TEST_DIR/sui_config/network.yaml > /dev/null 2>&1 &

sleep 1

echo "deploying wormhole contracts to localnet"
bash $DEPENDENCIES_DIR/scripts/deploy.sh devnet \
    -k ACMS4emBUzUD0vcYoiSM2Z8i2qs4MMrKeFRZY3L/pXYK

echo "deploying example coins"
worm sui deploy \
    $DEPENDENCIES_DIR/../contracts/example_coins \
    -n devnet -k ACMS4emBUzUD0vcYoiSM2Z8i2qs4MMrKeFRZY3L/pXYK

## run environment check here
npx ts-mocha -t 1000000 $TEST_DIR/00_environment.ts

# ## deploy scaffolding contracts
# echo "deploying scaffolding examples"
# yarn deploy contracts/02_token_bridge_relayer \
#     -c ts/tests/sui_config/client.yaml \
#     -m contracts/02_token_bridge_relayer/Move.localnet.toml

# ## run contract tests here
# npx ts-mocha -t 1000000 $TEST_DIR/0[1-9]*.ts

# nuke
pkill sui

# remove databases generated by localnet
rm -rf $SUI_CONFIG/*_db
