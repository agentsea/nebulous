#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# S3 URL for the binary
S3_BINARY_URL="https://nebulous-artifacts.s3.amazonaws.com/releases/latest/nebulous-latest-linux-amd64.tar.gz"
# Where to install the binary
INSTALL_DIR="/usr/local/bin"
BINARY_NAME="nebu"
TAR_FILENAME="nebulous-latest-linux-amd64.tar.gz"
ORIGINAL_BINARY_NAME="nebulous"

echo -e "${YELLOW}Starting installation...${NC}"

# Function to detect OS
detect_os() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        OS=$NAME
        VER=$VERSION_ID
    elif type lsb_release >/dev/null 2>&1; then
        OS=$(lsb_release -si)
        VER=$(lsb_release -sr)
    elif [ -f /etc/lsb-release ]; then
        . /etc/lsb-release
        OS=$DISTRIB_ID
        VER=$DISTRIB_RELEASE
    elif [ -f /etc/debian_version ]; then
        OS="Debian"
        VER=$(cat /etc/debian_version)
    else
        OS=$(uname -s)
        VER=$(uname -r)
    fi
    
    echo -e "${GREEN}Detected OS: $OS $VER${NC}"
}

# Function to check and install curl
install_curl() {
    if command -v curl >/dev/null 2>&1; then
        echo -e "${GREEN}curl is already installed.${NC}"
    else
        echo -e "${YELLOW}curl not found. Installing...${NC}"
        
        case "$OS" in
            "Ubuntu"|"Debian"|"Linux Mint")
                sudo apt-get update
                sudo apt-get install -y curl
                ;;
            "Fedora"|"CentOS"|"Red Hat Enterprise Linux")
                sudo dnf install -y curl || sudo yum install -y curl
                ;;
            "Arch Linux")
                sudo pacman -Sy curl
                ;;
            "Alpine Linux")
                apk add --no-cache curl
                ;;
            "macOS"|"Darwin")
                if command -v brew >/dev/null 2>&1; then
                    brew install curl
                else
                    echo -e "${RED}Homebrew not found. Please install Homebrew first: https://brew.sh/${NC}"
                    exit 1
                fi
                ;;
            *)
                echo -e "${RED}Unsupported OS for automatic curl installation. Please install curl manually.${NC}"
                exit 1
                ;;
        esac
        
        if command -v curl >/dev/null 2>&1; then
            echo -e "${GREEN}curl installed successfully.${NC}"
        else
            echo -e "${RED}Failed to install curl. Please install it manually.${NC}"
            exit 1
        fi
    fi
}

# Function to check and install rclone
install_rclone() {
    if command -v rclone >/dev/null 2>&1; then
        echo -e "${GREEN}rclone is already installed.${NC}"
    else
        echo -e "${YELLOW}rclone not found. Installing...${NC}"
        
        # Using rclone's install script which works across platforms
        curl https://rclone.org/install.sh | sudo bash
        
        if command -v rclone >/dev/null 2>&1; then
            echo -e "${GREEN}rclone installed successfully.${NC}"
        else
            echo -e "${RED}Failed to install rclone. Please install it manually.${NC}"
            exit 1
        fi
    fi
}

# Function to download and install the binary from S3
install_binary() {
    echo -e "${YELLOW}Downloading binary from S3...${NC}"
    
    # Create temp directory
    TMP_DIR=$(mktemp -d)
    
    # Download the tar file
    if ! curl -L "$S3_BINARY_URL" -o "$TMP_DIR/$TAR_FILENAME"; then
        echo -e "${RED}Failed to download binary from S3.${NC}"
        rm -rf "$TMP_DIR"
        exit 1
    fi
    
    # Extract the tar file
    echo -e "${YELLOW}Extracting tar file...${NC}"
    tar -xzf "$TMP_DIR/$TAR_FILENAME" -C "$TMP_DIR"
    
    # Make it executable
    chmod +x "$TMP_DIR/$ORIGINAL_BINARY_NAME"
    
    # Move to install directory with new name
    echo -e "${YELLOW}Installing binary to $INSTALL_DIR as $BINARY_NAME...${NC}"
    sudo mv "$TMP_DIR/$ORIGINAL_BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
    
    # Clean up
    rm -rf "$TMP_DIR"
    
    # Verify installation
    if command -v "$BINARY_NAME" >/dev/null 2>&1 || [ -f "$INSTALL_DIR/$BINARY_NAME" ]; then
        echo -e "${GREEN}Binary installed successfully to $INSTALL_DIR/$BINARY_NAME${NC}"
    else
        echo -e "${RED}Failed to install binary. Please check permissions and try again.${NC}"
        exit 1
    fi
}

# Main execution
detect_os
install_curl
install_rclone
install_binary

echo -e "${GREEN}Installation completed successfully!${NC}"