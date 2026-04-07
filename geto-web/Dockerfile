# Kenjaku geto-web: static phone-frame UI served by nginx.
FROM nginx:1.27-alpine

# Drop the default site config
RUN rm /etc/nginx/conf.d/default.conf

# Copy our config + static assets
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY index.html /usr/share/nginx/html/index.html
COPY styles.css /usr/share/nginx/html/styles.css
COPY app.js /usr/share/nginx/html/app.js
COPY enso.svg /usr/share/nginx/html/enso.svg
COPY fonts /usr/share/nginx/html/fonts

EXPOSE 80

HEALTHCHECK --interval=15s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -q -O /dev/null http://localhost/ || exit 1
