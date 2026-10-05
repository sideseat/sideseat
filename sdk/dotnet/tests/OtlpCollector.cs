using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading.Tasks;

namespace SideSeat.Tests;

/// <summary>A local OTLP/HTTP receiver that records every request and answers 200.</summary>
internal sealed class OtlpCollector : IDisposable
{
    private readonly HttpListener _listener = new HttpListener();
    private readonly ConcurrentQueue<Request> _requests = new ConcurrentQueue<Request>();

    public OtlpCollector()
    {
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        var port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        Endpoint = $"http://127.0.0.1:{port}";
        _listener.Prefixes.Add($"{Endpoint}/");
        _listener.Start();
        _ = Task.Run(Serve);
    }

    public string Endpoint { get; }

    /// <summary>
    /// The requests answered so far. A request is recorded before its response is sent, so every
    /// export that has returned is included.
    /// </summary>
    public IReadOnlyList<Request> Requests => _requests.ToList();

    public void Dispose() => _listener.Close();

    private async Task Serve()
    {
        while (_listener.IsListening)
        {
            HttpListenerContext context;
            try
            {
                context = await _listener.GetContextAsync();
            }
            catch (Exception error) when (error is HttpListenerException or ObjectDisposedException)
            {
                return;
            }
            using var body = new MemoryStream();
            await context.Request.InputStream.CopyToAsync(body);
            _requests.Enqueue(new Request(
                context.Request.Url!.AbsolutePath,
                context.Request.Headers["Authorization"],
                context.Request.Headers["x-team"],
                // Protobuf stores strings as raw UTF-8, so a decoded body still contains them.
                Encoding.UTF8.GetString(body.ToArray())));
            context.Response.StatusCode = 200;
            context.Response.Close();
        }
    }

    internal sealed record Request(string Path, string? Authorization, string? Team, string Body);
}
