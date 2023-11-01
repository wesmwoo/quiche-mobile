import time
import subprocess
import random

def send_quic_request(f):
    print(time.time(), 'get request sent')
    # get = subprocess.Popen(['cargo', 'run', '--bin', 'quiche-client', '--', 'https://192.168.1.45:4433', '--no-verify'],  stdout=f, stderr=subprocess.STDOUT)
    # get = subprocess.Popen(['cargo', 'run', '--bin', 'quiche-client', '--', 'https://www.cloudflare.com/page-data/sq/d/3050177178.json', '--no-verify'],  stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT) 
    get = subprocess.Popen(['cargo', 'run', '--bin', 'quiche-client', '--', 'https://172.29.72.153:4433', '--no-verify'],  stdout=f, stderr=subprocess.STDOUT)
    # get = subprocess.Popen(['google-chrome', '--enable-quic', '--origin-to-force-quic-on=192.168.4.229:4433', 'https://192.168.4.229:4433/'], stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    return get

def switch_ip(f, ip):
    t = send_quic_request(f)
    # time.sleep(.7) # for google chrome client
    time.sleep(0.25)
    print(time.time(), 'network switch begins')
    # switch = subprocess.run(['sudo', 'ifconfig', 'enp0s8', ip, 'netmask', '255.255.255.0', 'broadcast', '192.168.1.255']) 
    switch = subprocess.run(['sudo', 'ifconfig', 'enp0s8', ip, 'netmask', '255.255.128.0', 'broadcast', '172.29.127.255'])
    
    print(time.time(), 'network switch completed')
    return t

if __name__ == '__main__':
    with open("stdout.txt", "wb") as out:
        test_sudo = subprocess.run(['sudo', 'echo', 'hello'])
        new_ip = '172.29.73.{}'.format(random.randint(2,254)) 
        # new_ip = '192.168.1.{}'.format(random.randint(2,254))
        print('new_ip', new_ip)
        t = switch_ip(out, ip=new_ip)
        t.wait()
