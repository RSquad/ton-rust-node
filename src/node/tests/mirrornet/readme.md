# Prerequisites

- Python 3 (developed and tested with version 3.10.10)


## For Development

If you plan to modify the `mirrornet.py` script, it is recommended to install the following tools for code quality checks:

- black code formatter
```bash
pip install black
```
- ruff linter
```bash
pip install ruff
```
- pyright linter
```bash
pip install pyright
```


# Usage

Firstly run the script to generate blank config files for all nodes:
```bash
python mirrornet.py
```
Then fill in the generated `mirrornet.json` file with appropriate values.

All the nodes you added to the `mirrornet.json` have to be synced to the network you want to mirror. 
The script will stop all the nodes, generate new nodes and global configs, and run Mirrornet as a hardfork of the base network.

IP addresses in the config will be used for ssh connection to the nodes, and for generating DHT keys in the new global config.

It is recommended to use different ports for the new network, so add the `new_port` field to the config of each node with the port number for the new network. The script will replace the port in the node's config with the one you specified.

The hardfork tool must be present on the first node in the list, and the path to it must be specified in the config. The script will use it to generate a special hardfork block.

All the nodes will be validators of the new network.

Now run the script again to run Mirrornet:
```bash
python mirrornet.py
```


# Code Checks

After making changes to the `mirrornet.py` file, it is recommended to run linters and the code formatter to ensure code quality and consistency.

- Run linters:
```bash
ruff check mirrornet.py
pyright mirrornet.py
```

- Run code formatter:
```bash
black mirrornet.py
```