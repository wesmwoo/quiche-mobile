// Code taken from:
// - https://olegkutkov.me/2018/02/14/monitoring-linux-networking-state-using-netlink/
// - https://gist.github.com/hostilefork/f7cae3dc33e7416f2dd25a402857b6c6


#include <errno.h>
#include <stdio.h>
#include <memory.h>
#include <net/if.h>
#include <arpa/inet.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <linux/rtnetlink.h>

#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/un.h>
#include <pthread.h>

#define SOCKET_PATH "/tmp/netmon_pubsub_server"
#define MAX_CLIENTS 10
#define BUFFER_SIZE 256
#define WIRED_PRE "enx"
#define MOB_START "START"
#define MOB_END "END"


enum MobState { // should a timer be used to reset state?
    IDLE,
    START, // receives a new addr from enx**
    ONGOING, // for now, we use a simplified state machine model that doesn't track intermediate events.
    END, // receives a "delete route" event.
    // TODO: What if a delete route occurs, due to a previous link deactivation?
    //  How to ignore without knowing the route deleted? Keep two states?

    // below intermediate states are not used, yet
    // ROUTE_UPDATE_1, // should receive multiple route updates
    // ROUTE_UPDATE_2,
    // ROUTE_UPDATE_3,
    // ROUTE_UPDATE_4,
    // ROUTE_UPDATE_5, // unclear if 5 route updates always appear?
    // NEW_ADDR_2, // should receive a second new addr
    // NEW_ADDR_3, // sometimes, a third new addr is received?
    // NEW_ROUTE_5, 
};

enum MobEvent {
    NONE,
    DEL_ADDR,
    DEL_LINK,
    NEW_ADDR,
    NEW_LINK
};

struct Client {
    int socket;
    struct sockaddr_un address;
};

struct ThreadArgs {
    int server_socket;
    struct Client clients[MAX_CLIENTS];
    int num_clients;
};

void* handleNewClients(void* args) {
    struct ThreadArgs* thread_args = (struct ThreadArgs*)args;

    int server_socket = thread_args->server_socket;
    struct Client* clients = thread_args->clients;
    int* num_clients = &thread_args->num_clients;

    struct sockaddr_un client_address;
    socklen_t client_address_len = sizeof(client_address);

    while (1) {
        // Receive subscribe/unsubscribe message from a client
        char buffer[BUFFER_SIZE];
        int bytes_received = recvfrom(server_socket, buffer, BUFFER_SIZE, 0, (struct sockaddr*)&client_address, &client_address_len);

        if (bytes_received == -1) {
            perror("recvfrom");
            exit(EXIT_FAILURE);
        } else if (bytes_received == 0) {
            // Client disconnected, handle it gracefully
            printf("Client disconnected: %s\n", client_address.sun_path);

            // Unsubscribe client
            for (int i = 0; i < *num_clients; i++) {
                if (strcmp(client_address.sun_path, clients[i].address.sun_path) == 0) {
                    // Remove client from the array
                    for (int j = i; j < *num_clients - 1; j++) {
                        clients[j] = clients[j + 1];
                    }
                    (*num_clients)--;
                    break;
                }
            }
        } else {
            if (strncmp(buffer, "subscribe", 9) == 0) {
                // Subscribe client
                if (*num_clients < MAX_CLIENTS) {
                    printf("Client subscribed: %s\n", client_address.sun_path);
                    clients[*num_clients].socket = server_socket; // Reuse server socket for broadcasting
                    clients[*num_clients].address = client_address;
                    (*num_clients)++;
                }
            } else if (strncmp(buffer, "unsubscribe", 11) == 0) {
                // Unsubscribe client
                for (int i = 0; i < *num_clients; i++) {
                    if (strcmp(client_address.sun_path, clients[i].address.sun_path) == 0) {
                        printf("Client unsubscribed: %s\n", client_address.sun_path);
                        // Remove client from the array
                        for (int j = i; j < *num_clients - 1; j++) {
                            clients[j] = clients[j + 1];
                        }
                        (*num_clients)--;
                        break;
                    }
                }
            }
        }
    }

    return NULL;
}


// little helper to parsing message using netlink macroses
void parseRtattr(struct rtattr *tb[], int max, struct rtattr *rta, int len)
{
    memset(tb, 0, sizeof(struct rtattr *) * (max + 1));

    while (RTA_OK(rta, len)) {  // while not end of the message
        if (rta->rta_type <= max) {
            tb[rta->rta_type] = rta; // read attr
        }
        rta = RTA_NEXT(rta,len);    // get next attr
    }
}

int main(int argc, char *argv[])
{
    // Remove the existing socket file, if any
    unlink(SOCKET_PATH);

    int server_socket;
    struct sockaddr_un server_address;

    // Create a Unix domain datagram socket
    if ((server_socket = socket(AF_UNIX, SOCK_DGRAM, 0)) == -1) {
        perror("socket");
        exit(EXIT_FAILURE);
    }

    // Set up server address
    memset(&server_address, 0, sizeof(server_address));
    server_address.sun_family = AF_UNIX;
    strcpy(server_address.sun_path, SOCKET_PATH);

    // Bind the socket to the server address
    if (bind(server_socket, (struct sockaddr*)&server_address, sizeof(server_address)) == -1) {
        perror("bind");
        exit(EXIT_FAILURE);
    }

    // Initialize thread arguments
    struct ThreadArgs thread_args;
    thread_args.server_socket = server_socket;
    thread_args.num_clients = 0;
    struct Client* clients = thread_args.clients;


    // Create threads for handling new clients and sending messages to subscribers
    pthread_t newClientsThread;

    if (pthread_create(&newClientsThread, NULL, handleNewClients, (void*)&thread_args) != 0) {
        perror("pthread_create");
        exit(EXIT_FAILURE);
    }

    // Next, set up the netlink socket
    int nl_fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_ROUTE);

    if (nl_fd < 0) {
        printf("Failed to create netlink socket: %s\n", (char*)strerror(errno));
        return 1;
    }

    struct sockaddr_nl  local;  // local addr struct
    char buf[8192];             // message buffer
    struct iovec iov;           // message structure
    iov.iov_base = buf;         // set message buffer as io
    iov.iov_len = sizeof(buf);  // set size

    int nbytes; // number of bytes sent
    char test[] = "test";

    memset(&local, 0, sizeof(local));

    local.nl_family = AF_NETLINK;       // set protocol family
    local.nl_groups =   RTMGRP_LINK | RTMGRP_IPV4_IFADDR | RTMGRP_IPV4_ROUTE;   // set groups we interested in
    local.nl_pid = getpid();    // set out id using current process id

    // initialize protocol message header
    struct msghdr msg;  
    {
        msg.msg_name = &local;                  // local address
        msg.msg_namelen = sizeof(local);        // address size
        msg.msg_iov = &iov;                     // io vector
        msg.msg_iovlen = 1;                     // io size
    }   

    if (bind(nl_fd, (struct sockaddr*)&local, sizeof(local)) < 0) {     // bind socket
        printf("Failed to bind netlink socket: %s\n", (char*)strerror(errno));
        close(nl_fd);
        return 1;
    }

    enum MobState mob_state = IDLE;
    enum MobEvent mob_flag;

    // read and parse all messages from the
    while (1) {
        mob_flag = NONE;

        ssize_t status = recvmsg(nl_fd, &msg, MSG_DONTWAIT);

        //  check status
        if (status < 0) {
            if (errno == EINTR || errno == EAGAIN)
            {
                usleep(250000);
                continue;
            }

            printf("Failed to read netlink: %s", (char*)strerror(errno));
            continue;
        }

        if (msg.msg_namelen != sizeof(local)) { // check message length, just in case
            printf("Invalid length of the sender address struct\n");
            continue;
        }

        // message parser
        struct nlmsghdr *h;
        char *pIfName;
        char *upFlag;

        for (h = (struct nlmsghdr*)buf; status >= (ssize_t)sizeof(*h); ) {   // read all messagess headers
            int len = h->nlmsg_len;
            int l = len - sizeof(*h);
            char *ifName;

            if ((l < 0) || (len > status)) {
                printf("Invalid message length: %i\n", len);
                continue;
            }

            // now we can check message type
            if ((h->nlmsg_type == RTM_NEWROUTE) || (h->nlmsg_type == RTM_DELROUTE)) { // some changes in routing table
                printf("Routing table was changed\n");
                if (h->nlmsg_type == RTM_NEWROUTE) {
                    printf("NEWROUTE\n");
                } else if (h->nlmsg_type == RTM_DELROUTE) {
                    printf("DELROUTE\n");
                    if (mob_state == ONGOING) {
                        printf("MOB STATE ONGOING->END\n");
                        mob_state = END;
                    }
                }
            } else {    // in other case we need to go deeper
                char *ifUpp;
                char *ifRunn;
                struct ifinfomsg *ifi;  // structure for network interface info
                struct rtattr *tb[IFLA_MAX + 1];

                ifi = (struct ifinfomsg*) NLMSG_DATA(h);    // get information about changed network interface

                parseRtattr(tb, IFLA_MAX, IFLA_RTA(ifi), h->nlmsg_len);  // get attributes
                
                if (tb[IFLA_IFNAME]) {  // validation
                    ifName = (char*)RTA_DATA(tb[IFLA_IFNAME]); // get network interface name
                }

                if (ifi->ifi_flags & IFF_UP) { // get UP flag of the network interface
                    ifUpp = (char*)"UP";
                } else {
                    ifUpp = (char*)"DOWN";
                }

                if (ifi->ifi_flags & IFF_RUNNING) { // get RUNNING flag of the network interface
                    ifRunn = (char*)"RUNNING";
                } else {
                    ifRunn = (char*)"NOT RUNNING";
                }

                char ifAddress[256];    // network addr
                struct ifaddrmsg *ifa; // structure for network interface data
                struct rtattr *tba[IFA_MAX+1];

                ifa = (struct ifaddrmsg*)NLMSG_DATA(h); // get data from the network interface

                parseRtattr(tba, IFA_MAX, IFA_RTA(ifa), h->nlmsg_len);

                if (tba[IFA_LOCAL]) {
                    inet_ntop(AF_INET, RTA_DATA(tba[IFA_LOCAL]), ifAddress, sizeof(ifAddress)); // get IP addr
                }

                switch (h->nlmsg_type) { // what is actually happenned?
                    case RTM_DELADDR:
                        printf("Interface %s: address was removed\n", ifName);
                        // nbytes = sendto(bc_fd, ifName, strlen(ifName), 0, (struct sockaddr*) &addr, sizeof(addr));
                        // if (nbytes < 0) {
                        //     printf("Error in multicast\n");
                        // }
                        // printf("sent DEL ADDR");
                        mob_flag = DEL_ADDR;
                        pIfName = ifName;
                        break;

                    case RTM_DELLINK:
                        printf("Network interface %s was removed\n", ifName);
                        // nbytes = sendto(bc_fd, ifName, strlen(ifName), 0, (struct sockaddr*) &addr, sizeof(addr));
                        // if (nbytes < 0) {
                        //     printf("Error in multicast\n");
                        // }
                        // printf("sent DEL LINK");
                        mob_flag = DEL_LINK;
                        pIfName = ifName;
                        break;

                    case RTM_NEWLINK:
                        printf("New network interface %s, state: %s %s\n", ifName, ifUpp, ifRunn);
                        // nbytes = sendto(bc_fd, ifName, strlen(ifName), 0, (struct sockaddr*) &addr, sizeof(addr));
                        // if (nbytes < 0) {
                        //     printf("Error in multicast\n");
                        // }
                        // printf("sent NEW LINK");
                        mob_flag = NEW_LINK;
                        pIfName = ifName;
                        upFlag = ifUpp;
                        break;

                    case RTM_NEWADDR:
                        printf("Interface %s: new address was assigned: %s\n", ifName, ifAddress);
                        // nbytes = sendto(bc_fd, ifAddress, strlen(ifAddress), 0, (struct sockaddr*) &addr, sizeof(addr));
                        // if (nbytes < 0) {
                        //     printf("Error in multicast\n");
                        // }
                        mob_flag = NEW_ADDR;
                        pIfName = ifName;
                        break;
                }
            }

            status -= NLMSG_ALIGN(len); // align offsets by the message length, this is important

            h = (struct nlmsghdr*)((char*)h + NLMSG_ALIGN(len));    // get next message
        }

        switch (mob_flag) {
            case NONE:
                break;
            case DEL_ADDR:
                printf("DEL_ADDR\n");
                strcat(pIfName, " 1");
                break;
            case DEL_LINK:
                printf("DEL_LINK\n");
                strcat(pIfName, " 2");
                break;
            case NEW_ADDR:
                printf("NEW_ADDR\n");
                strcat(pIfName, " 3");
                break;
            case NEW_LINK:
                printf("NEW_LINK\n");
                strcat(pIfName, " 4");
                if (strncmp(WIRED_PRE, pIfName, 3) == 0 && mob_state == IDLE) {
                    if (strcmp(upFlag, "UP") == 0) {
                        mob_state = START;
                        printf("MOB STATE IDLE->START\n");
                    } else if (strcmp(upFlag, "DOWN") == 0) {
                        printf("[MOBILITY EVENT] interface down\n");
                    }
                }
                break;
            default:
                break;
        }

        if (mob_flag != NONE && mob_state == IDLE) {
            for (int i = 0; i < thread_args.num_clients; i++) {
                if (sendto(clients[i].socket, pIfName, strlen(pIfName), 0, (struct sockaddr*)&clients[i].address, sizeof(clients[i].address)) == -1) {
                    perror("sendto");
                    // TODO: How to distinguish between an error and client disconnection?
                    // For now, we assume client disconnected, handle it gracefully
                    printf("Client disconnected: %s\n", clients[i].address.sun_path);

                    // Remove client from the array
                    for (int j = i; j < thread_args.num_clients - 1; j++) {
                        clients[j] = clients[j + 1];
                    }
                    (thread_args.num_clients)--;
                    // exit(EXIT_FAILURE);
                }
            }
        }

        if (mob_state == START) {
            for (int i = 0; i < thread_args.num_clients; i++) {
                if (sendto(clients[i].socket, MOB_START, strlen(MOB_START), 0, (struct sockaddr*)&clients[i].address, sizeof(clients[i].address)) == -1) {
                    perror("sendto");
                    // TODO: How to distinguish between an error and client disconnection?
                    // For now, we assume client disconnected, handle it gracefully
                    printf("Client disconnected: %s\n", clients[i].address.sun_path);

                    // Remove client from the array
                    for (int j = i; j < thread_args.num_clients - 1; j++) {
                        clients[j] = clients[j + 1];
                    }
                    (thread_args.num_clients)--;
                    // exit(EXIT_FAILURE);
                }
            }

            printf("[MOBILITY EVENT] START\n");
            mob_state = ONGOING;
            printf("MOB STATE START->ONGOING\n");

        } else if (mob_state == END) {
            for (int i = 0; i < thread_args.num_clients; i++) {
                if (sendto(clients[i].socket, MOB_END, strlen(MOB_END), 0, (struct sockaddr*)&clients[i].address, sizeof(clients[i].address)) == -1) {
                    perror("sendto");
                    // TODO: How to distinguish between an error and client disconnection?
                    // For now, we assume client disconnected, handle it gracefully
                    printf("Client disconnected: %s\n", clients[i].address.sun_path);

                    // Remove client from the array
                    for (int j = i; j < thread_args.num_clients - 1; j++) {
                        clients[j] = clients[j + 1];
                    }
                    (thread_args.num_clients)--;
                    // exit(EXIT_FAILURE);
                }
            }

            printf("[MOBILITY EVENT] END\n");
            mob_state = IDLE; // reset mobility state
            printf("MOB STATE END->IDLE\n");
        }

        usleep(250000); // sleep for a while
    }

    pthread_join(newClientsThread, NULL); // shouldn't get here

    close(nl_fd);  // close socket

    return 0;
}